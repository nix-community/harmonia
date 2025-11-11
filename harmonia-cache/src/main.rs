#![warn(clippy::dbg_macro)]

use actix_web::middleware;
use config::Config;
use error::{CacheError, IoErrorContext, Result, StoreError};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::{fmt::Display, time::Duration};
use url::Url;

use actix_web::{App, HttpResponse, HttpServer, http, web};
use harmonia_store_remote_legacy::protocol::StorePath;

/// Macro for building byte vectors efficiently from parts
#[macro_export]
macro_rules! build_bytes {
    ($($part:expr),* $(,)?) => {{
        let parts: &[&[u8]] = &[$($part),*];
        let capacity = parts.iter().map(|p| p.len()).sum();
        let mut result = Vec::with_capacity(capacity);
        for part in parts {
            result.extend_from_slice(part);
        }
        result
    }};
}

mod buildlog;
mod cacheinfo;
mod config;
mod error;
mod health;
mod nar;
mod narinfo;
mod narlist;
mod prometheus;
mod root;
mod serve;
mod store;
mod template;
mod tls;
mod version;

async fn nixhash(settings: &web::Data<Config>, hash: &[u8]) -> Result<Option<StorePath>> {
    let mut daemon_guard =
        settings
            .store
            .get_daemon()
            .await
            .map_err(|e| StoreError::Operation {
                reason: format!("Failed to get daemon connection: {e}"),
            })?;
    let daemon = daemon_guard.as_mut().unwrap();

    Ok(daemon
        .query_path_from_hash_part(hash)
        .await
        .map_err(|e| StoreError::PathQuery {
            hash: String::from_utf8_lossy(hash).to_string(),
            reason: e.to_string(),
        })?)
}

const TAILWIND_CSS: &str = include_str!("styles/output.css");

const CARGO_NAME: &str = env!("CARGO_PKG_NAME");
const CARGO_VERSION: &str = env!("CARGO_PKG_VERSION");
const CARGO_HOME_PAGE: &str = env!("CARGO_PKG_HOMEPAGE");
const NIXBASE32_ALPHABET: &str = "0123456789abcdfghijklmnpqrsvwxyz";

fn cache_control_max_age(max_age: u32) -> http::header::CacheControl {
    http::header::CacheControl(vec![http::header::CacheDirective::MaxAge(max_age)])
}

fn cache_control_max_age_1y() -> http::header::CacheControl {
    cache_control_max_age(365 * 24 * 60 * 60)
}

fn cache_control_max_age_1d() -> http::header::CacheControl {
    cache_control_max_age(24 * 60 * 60)
}

fn cache_control_no_store() -> http::header::CacheControl {
    http::header::CacheControl(vec![http::header::CacheDirective::NoStore])
}

macro_rules! some_or_404 {
    ($res:expr) => {
        match $res {
            Some(val) => val,
            None => {
                return Ok(HttpResponse::NotFound()
                    .insert_header(crate::cache_control_no_store())
                    .body("missed hash"))
            }
        }
    };
}
pub(crate) use some_or_404;

#[derive(Debug)]
struct ServerError {
    err: CacheError,
}

impl Display for ServerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.err)
    }
}

impl actix_web::error::ResponseError for ServerError {
    fn status_code(&self) -> actix_web::http::StatusCode {
        use actix_web::http::StatusCode;
        match &self.err {
            CacheError::Config(_) => StatusCode::INTERNAL_SERVER_ERROR,
            CacheError::Store(StoreError::PathQuery { .. }) => StatusCode::NOT_FOUND,
            CacheError::Signing(_) => StatusCode::INTERNAL_SERVER_ERROR,
            CacheError::Serve(_) => StatusCode::INTERNAL_SERVER_ERROR,
            CacheError::Nar(_) => StatusCode::INTERNAL_SERVER_ERROR,
            CacheError::BuildLog(_) => StatusCode::INTERNAL_SERVER_ERROR,
            CacheError::NarInfo(_) => StatusCode::INTERNAL_SERVER_ERROR,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl From<CacheError> for ServerError {
    fn from(err: CacheError) -> ServerError {
        ServerError { err }
    }
}

type ServerResult = std::result::Result<HttpResponse, ServerError>;

async fn inner_main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let mut config = config::load()?;

    // Initialize metrics with config
    let metrics = prometheus::initialize_metrics(&mut config)?;

    let c = web::Data::new(config);
    let config_data = c.clone();
    let metrics_data = web::Data::new(metrics.clone());

    log::info!("listening on {}", c.bind);
    let mut server = HttpServer::new(move || {
        App::new()
                .wrap(middleware::Condition::new(config_data.enable_compression, middleware::Compress::default()))
                .wrap(prometheus::PrometheusMiddleware::new(metrics.clone()))
                .app_data(config_data.clone())
                .app_data(metrics_data.clone())
                .route("/", web::get().to(root::get))
                .route("/{hash}.ls", web::get().to(narlist::get))
                .route("/{hash}.ls", web::head().to(narlist::get))
                .route("/{hash}.narinfo", web::get().to(narinfo::get))
                .route("/{hash}.narinfo", web::head().to(narinfo::get))
                .route(
                    &format!("/nar/{{narhash:[{NIXBASE32_ALPHABET}]{{52}}}}.nar"),
                    web::get().to(nar::get),
                )
                .route(
                    // narinfos served by nix-serve have the narhash embedded in the nar URL.
                    // While we don't do that, if nix-serve is replaced with harmonia, the old nar URLs
                    // will stay in client caches for a while - so support them anyway.
                    &format!(
                        "/nar/{{outhash:[{NIXBASE32_ALPHABET}]{{32}}}}-{{narhash:[{NIXBASE32_ALPHABET}]{{52}}}}.nar"
                    ),
                    web::get().to(nar::get),
                )
                .route("/serve/{hash}{path:.*}", web::get().to(serve::get))
                .route("/log/{drv}", web::get().to(buildlog::get))
                .route("/version", web::get().to(version::get))
                .route("/health", web::get().to(health::get))
                .route("/nix-cache-info", web::get().to(cacheinfo::get))
                .route("/metrics", web::get().to(prometheus::metrics_handler))
        })
        // default is 5 seconds, which is too small when doing mass requests on slow machines
        .client_request_timeout(Duration::from_secs(30))
    .workers(c.workers)
    .max_connection_rate(c.max_connection_rate);

    let try_url = Url::parse(&c.bind);
    let (bind, uds) = if let Ok(url) = try_url.as_ref() {
        if url.scheme() != "unix" {
            (c.bind.as_str(), false)
        } else if url.host().is_none() {
            (url.path(), true)
        } else {
            return Err(error::ServerError::Startup {
                reason: "Can only bind to file URLs without host portion.".to_string(),
            }
            .into());
        }
    } else {
        (c.bind.as_str(), false)
    };

    if c.tls_cert_path.is_some() || c.tls_key_path.is_some() {
        if uds {
            log::error!("TLS is not supported with Unix domain sockets.");
            std::process::exit(1);
        }
        let config = tls::load_tls_config(
            Path::new(&c.tls_cert_path.clone().unwrap()),
            Path::new(&c.tls_key_path.clone().unwrap()),
        )?;

        server = server
            .bind_rustls_0_23(c.bind.clone(), config)
            .io_context("Failed to bind with TLS")?;
    } else if uds {
        if !cfg!(unix) {
            log::error!("Binding to Unix domain sockets is only supported on Unix.");
            std::process::exit(1);
        } else {
            let socket_path = Path::new(bind);
            server = server
                .bind_uds(socket_path)
                .io_context("Failed to bind to Unix domain socket")?;
            fs::set_permissions(socket_path, fs::Permissions::from_mode(0o777))
                .io_context("Failed to set socket permissions")?;
        }
    } else {
        server = server
            .bind(c.bind.clone())
            .io_context("Failed to bind server")?;
    }

    server.run().await.io_context("Failed to start server")
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    inner_main().await.map_err(std::io::Error::other)
}
