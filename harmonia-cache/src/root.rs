use std::collections::HashMap;
use std::error::Error;

use actix_web::{HttpRequest, HttpResponse, http, web};

use crate::TAILWIND_CSS;
use crate::template::{LANDING_TEMPLATE, LANDING_WITH_KEYS_TEMPLATE, render, render_page};
use crate::{CARGO_HOME_PAGE, CARGO_NAME, CARGO_VERSION, config};

pub(crate) async fn get(
    req: HttpRequest,
    config: web::Data<config::Config>,
) -> Result<HttpResponse, Box<dyn Error>> {
    let mut vars = HashMap::new();
    vars.insert("version", CARGO_VERSION.to_string());
    vars.insert(
        "store",
        String::from_utf8_lossy(config.store.virtual_store()).to_string(),
    );
    vars.insert("priority", config.priority.to_string());
    vars.insert("homepage", CARGO_HOME_PAGE.to_string());
    vars.insert("name", CARGO_NAME.to_string());

    // Determine scheme: check X-Forwarded-Proto first, then connection info, default to https
    let scheme = req
        .headers()
        .get("X-Forwarded-Proto")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| req.connection_info().scheme().to_string());

    // Get cache URL from Host header
    let host = req
        .headers()
        .get("Host")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("cache.example.com");
    let cache_url = format!("{scheme}://{host}");
    vars.insert("cache_url", cache_url);

    // Get public keys from configured signing keys
    let public_keys: Vec<String> = config
        .secret_keys
        .iter()
        .map(|sk| sk.to_public_key().to_string())
        .collect();

    // Choose template based on whether keys are configured
    let template = if public_keys.is_empty() {
        LANDING_TEMPLATE
    } else {
        // Space-separated keys for CLI/nix.conf usage
        vars.insert("public_keys_cli", public_keys.join(" "));
        // Quoted keys for Nix list literals
        vars.insert(
            "public_keys_list",
            public_keys
                .iter()
                .map(|k| format!("\"{k}\""))
                .collect::<Vec<_>>()
                .join(" "),
        );
        LANDING_WITH_KEYS_TEMPLATE
    };

    let content = render(template, vars);
    let html = render_page(
        &format!("Nix Binary Cache - {CARGO_NAME} {CARGO_VERSION}"),
        TAILWIND_CSS,
        &content,
    );

    Ok(HttpResponse::Ok()
        .insert_header(http::header::ContentType(mime::TEXT_HTML_UTF_8))
        .body(html))
}
