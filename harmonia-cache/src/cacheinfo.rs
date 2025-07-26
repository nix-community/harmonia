use std::error::Error;

use crate::config;
use actix_web::{HttpResponse, http, web};

pub(crate) async fn get(config: web::Data<config::Config>) -> Result<HttpResponse, Box<dyn Error>> {
    let priority_str = config.priority.to_string();

    let body = crate::build_bytes!(
        b"StoreDir: ",
        config.store.virtual_store(),
        b"\nWantMassQuery: 1\nPriority: ",
        priority_str.as_bytes(),
        b"\n"
    );

    Ok(HttpResponse::Ok()
        .insert_header((http::header::CONTENT_TYPE, "text/x-nix-cache-info"))
        .body(body))
}
