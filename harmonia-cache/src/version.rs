use std::error::Error;

use actix_web::HttpResponse;

pub(crate) async fn get() -> Result<HttpResponse, Box<dyn Error>> {
    Ok(HttpResponse::Ok().body(format!(
        "{} {}",
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION")
    )))
}
