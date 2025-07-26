use std::error::Error;

use actix_web::HttpResponse;

pub(crate) async fn get() -> Result<HttpResponse, Box<dyn Error>> {
    Ok(HttpResponse::Ok().body("OK\n"))
}
