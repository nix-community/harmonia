//! Handler for content-addressed derivation realisations (build-trace).
//!
//! Serves `UnkeyedRealisation` JSON at
//! `/build-trace-v2/{drv_path}/{output}.doi`, where `{drv_path}` is the store
//! path base name of the derivation and `{output}` is the output name.

use actix_web::{HttpResponse, web};
use harmonia_store_core::derived_path::OutputName;
use harmonia_store_core::realisation::UnkeyedRealisation;
use harmonia_store_core::signature::Signature;
use harmonia_store_core::store_path::StorePath;

use crate::config::Config;
use crate::{ServerResult, cache_control_max_age_1y, cache_control_no_store};

pub async fn get(path: web::Path<(String, String)>, settings: web::Data<Config>) -> ServerResult {
    let (drv_path_raw, output_raw) = path.into_inner();
    let output_raw = output_raw
        .strip_suffix(".doi")
        .unwrap_or(&output_raw)
        .to_owned();

    // Validate inputs so garbage URLs are 4xx, not a db query.
    let drv_path: StorePath = match drv_path_raw.parse() {
        Ok(p) => p,
        Err(e) => {
            tracing::debug!("Invalid drv path '{drv_path_raw}': {e}");
            return Ok(HttpResponse::BadRequest()
                .insert_header(cache_control_no_store())
                .body(format!("Invalid derivation path: {e}")));
        }
    };
    let _output_name: OutputName = match output_raw.parse() {
        Ok(o) => o,
        Err(e) => {
            tracing::debug!("Invalid output name '{output_raw}': {e}");
            return Ok(HttpResponse::BadRequest()
                .insert_header(cache_control_no_store())
                .body(format!("Invalid output name: {e}")));
        }
    };

    // BuildTraceV3 keys on the *base name* of the drv path, which is exactly
    // the StorePath display form (hash-name.drv).
    let row = settings
        .store
        .query_realisation(&drv_path.to_string(), &output_raw)?;

    let row = match row {
        Some(r) => r,
        None => {
            tracing::debug!("Realisation not found for {drv_path}!{output_raw}");
            return Ok(HttpResponse::NotFound()
                .insert_header(cache_control_no_store())
                .body("realisation not found"));
        }
    };

    let out_path: StorePath = match settings.store.store_dir().parse(&row.output_path) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(
                "invalid output_path '{}' in BuildTraceV3: {e}",
                row.output_path
            );
            return Ok(HttpResponse::NotFound()
                .insert_header(cache_control_no_store())
                .body("realisation not found"));
        }
    };

    let signatures = match row
        .signatures
        .as_deref()
        .unwrap_or("")
        .split_whitespace()
        .map(|s| s.parse::<Signature>().map_err(|e| (s.to_owned(), e)))
        .collect::<std::result::Result<_, _>>()
    {
        Ok(sigs) => sigs,
        Err((sig, e)) => {
            tracing::warn!("invalid signature '{sig}' in BuildTraceV3: {e}");
            return Ok(HttpResponse::InternalServerError()
                .insert_header(cache_control_no_store())
                .body("invalid realisation signature"));
        }
    };

    let realisation = UnkeyedRealisation {
        out_path,
        signatures,
    };

    Ok(HttpResponse::Ok()
        .insert_header(cache_control_max_age_1y())
        .content_type("application/json")
        .json(realisation))
}
