use std::error::Error;

use crate::config;
use actix_web::{HttpResponse, http, web};
use harmonia_store_nar_info::CacheInfo;

pub(crate) async fn get(config: web::Data<config::Config>) -> Result<HttpResponse, Box<dyn Error>> {
    let body = CacheInfo {
        store_dir: Some(config.store.store_dir().clone()),
        want_mass_query: Some(true),
        priority: Some(i32::try_from(config.priority)?),
    }
    .to_string();

    Ok(HttpResponse::Ok()
        .insert_header((http::header::CONTENT_TYPE, "text/x-nix-cache-info"))
        .body(body))
}
