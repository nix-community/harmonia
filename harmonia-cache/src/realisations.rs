//! Handler for content-addressed derivation realisations (build-trace).
//!
//! Serves `UnkeyedRealisation` JSON at
//! `/build-trace-v2/{drv_path}/{output}.doi`, where `{drv_path}` is the store
//! path base name of the derivation and `{output}` is the output name.

use actix_web::{HttpResponse, web};
use harmonia_store_core::derived_path::OutputName;
use harmonia_store_core::realisation::DrvOutput;
use harmonia_store_core::store_path::StorePath;
use harmonia_store_remote::DaemonStore;

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
    let realisation = match guard.client().query_realisation(&drv_output).await {
        Ok(r) => r,
        Err(e) => {
            // Daemon may lack the `realisation-with-path-not-hash` feature
            // (e.g. talking to an older nix-daemon). Treat as not-found so
            // clients fall back gracefully.
            tracing::debug!(
                "Failed to query realisation for {drv_output}: {e} (treating as not found)"
            );
            return Ok(HttpResponse::NotFound()
                .insert_header(cache_control_no_store())
                .body("realisation not found"));
        }
    };

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
