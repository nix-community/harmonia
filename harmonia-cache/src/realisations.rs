//! Handler for content-addressed derivation realisations (build-trace).
//!
//! Serves `UnkeyedRealisation` JSON at
//! `/build-trace-v2/{drv_path}/{output}.doi`, where `{drv_path}` is the store
//! path base name of the derivation and `{output}` is the output name.

use actix_web::{HttpResponse, web};
use harmonia_store_core::derived_path::OutputName;
use harmonia_store_core::realisation::DrvOutput;
use harmonia_store_core::store_path::StorePath;
use harmonia_store_remote::{DaemonStore, FEATURE_REALISATION_WITH_PATH};

use crate::config::Config;
use crate::{ServerResult, cache_control_max_age_1y, cache_control_no_store};

pub async fn get(path: web::Path<(String, String)>, settings: web::Data<Config>) -> ServerResult {
    let (drv_path_raw, output_raw) = path.into_inner();
    let output_raw = output_raw
        .strip_suffix(".doi")
        .unwrap_or(&output_raw)
        .to_owned();

    let drv_path: StorePath = match drv_path_raw.parse() {
        Ok(p) => p,
        Err(e) => {
            tracing::debug!("Invalid drv path '{drv_path_raw}': {e}");
            return Ok(HttpResponse::BadRequest()
                .insert_header(cache_control_no_store())
                .body(format!("Invalid derivation path: {e}")));
        }
    };
    let output_name: OutputName = match output_raw.parse() {
        Ok(o) => o,
        Err(e) => {
            tracing::debug!("Invalid output name '{output_raw}': {e}");
            return Ok(HttpResponse::BadRequest()
                .insert_header(cache_control_no_store())
                .body(format!("Invalid output name: {e}")));
        }
    };

    let drv_output = DrvOutput {
        drv_path,
        output_name,
    };

    let mut guard = settings.store.acquire().await?;
    // Older nix-daemon without `realisation-with-path-not-hash` can't answer
    // this at all; degrade to 404 so clients fall back. Any other error from
    // the actual query is a real failure and must surface as 5xx.
    if !guard.client().has_feature(FEATURE_REALISATION_WITH_PATH) {
        tracing::debug!("Daemon missing {FEATURE_REALISATION_WITH_PATH}; returning 404");
        return Ok(HttpResponse::NotFound()
            .insert_header(cache_control_no_store())
            .body("realisation not found"));
    }
    let realisation = guard
        .client()
        .query_realisation(&drv_output)
        .await
        .map_err(crate::error::StoreError::from)
        .map_err(crate::error::CacheError::from)?;

    match realisation {
        Some(realisation) => Ok(HttpResponse::Ok()
            .insert_header(cache_control_max_age_1y())
            .content_type("application/json")
            .json(realisation)),
        None => {
            tracing::debug!("Realisation not found for {drv_output}");
            Ok(HttpResponse::NotFound()
                .insert_header(cache_control_no_store())
                .body("realisation not found"))
        }
    }
}
