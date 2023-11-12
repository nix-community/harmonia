use std::error::Error;

use crate::config;
use actix_web::{http, web, HttpResponse};

pub(crate) async fn get(config: web::Data<config::Config>) -> Result<HttpResponse, Box<dyn Error>> {
    Ok(HttpResponse::Ok()
        .insert_header((http::header::CONTENT_TYPE, "text/x-nix-cache-info"))
        .body(
            [
                format!("StoreDir: {}", libnixstore::get_store_dir()),
                "WantMassQuery: 1".to_owned(),
                format!("Priority: {}", config.priority),
                "".to_owned(),
            ]
            .join("\n"),
        ))
}
