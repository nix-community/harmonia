use std::collections::HashMap;
use std::error::Error;

use actix_web::{HttpResponse, http, web};

use crate::TAILWIND_CSS;
use crate::template::{LANDING_TEMPLATE, render, render_page};
use crate::{CARGO_HOME_PAGE, CARGO_NAME, CARGO_VERSION, config};

pub(crate) async fn get(config: web::Data<config::Config>) -> Result<HttpResponse, Box<dyn Error>> {
    let mut vars = HashMap::new();
    vars.insert("version", CARGO_VERSION.to_string());
    vars.insert(
        "store",
        String::from_utf8_lossy(config.store.virtual_store()).to_string(),
    );
    vars.insert("priority", config.priority.to_string());
    vars.insert("homepage", CARGO_HOME_PAGE.to_string());
    vars.insert("name", CARGO_NAME.to_string());

    let content = render(LANDING_TEMPLATE, vars);
    let html = render_page(
        &format!("Nix Binary Cache - {CARGO_NAME} {CARGO_VERSION}"),
        TAILWIND_CSS,
        &content,
    );

    Ok(HttpResponse::Ok()
        .insert_header(http::header::ContentType(mime::TEXT_HTML_UTF_8))
        .body(html))
}
