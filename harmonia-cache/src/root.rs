use std::collections::HashMap;
use std::error::Error;

use actix_web::{HttpRequest, HttpResponse, http, web};

use crate::TAILWIND_CSS;
use crate::template::{
    LANDING_TEMPLATE, LANDING_WITH_KEYS_TEMPLATE, html_escape, render, render_page,
};
use crate::{CARGO_HOME_PAGE, CARGO_NAME, CARGO_VERSION, config};

pub(crate) async fn get(
    req: HttpRequest,
    config: web::Data<config::Config>,
) -> Result<HttpResponse, Box<dyn Error>> {
    let mut vars = HashMap::new();
    vars.insert("version", CARGO_VERSION.to_string());
    vars.insert(
        "store",
        html_escape(&String::from_utf8_lossy(config.store.virtual_store())),
    );
    vars.insert("priority", config.priority.to_string());
    vars.insert("homepage", CARGO_HOME_PAGE.to_string());
    vars.insert("name", CARGO_NAME.to_string());

    // Scheme from X-Forwarded-Proto or connection info; both can be
    // client-controlled, so allowlist http/https.
    let scheme = req
        .headers()
        .get("X-Forwarded-Proto")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(|s| s.trim().to_ascii_lowercase())
        .unwrap_or_else(|| req.connection_info().scheme().to_ascii_lowercase());
    let scheme = match scheme.as_str() {
        "http" | "https" => scheme,
        _ if config.tls_cert_path.is_some() => "https".to_string(),
        _ => "http".to_string(),
    };

    // Host header is reflected into copy-paste config snippets; escape it.
    let host = req
        .headers()
        .get("Host")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("cache.example.com");
    let cache_url = format!("{scheme}://{host}");
    vars.insert("cache_url", html_escape(&cache_url));

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
        // Space-separated keys for CLI/nix.conf usage. Key names come from the
        // operator-supplied secret-key files, so escape defensively.
        vars.insert("public_keys_cli", html_escape(&public_keys.join(" ")));
        // Quoted keys for Nix list literals
        vars.insert(
            "public_keys_list",
            html_escape(
                &public_keys
                    .iter()
                    .map(|k| format!("\"{k}\""))
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
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
