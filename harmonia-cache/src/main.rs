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
use harmonia_store_core::store_path::{StorePath, StorePathHash};

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
mod realisations;
mod root;
mod serve;
mod store;
mod systemd;
mod template;
mod tls;
mod version;
mod zstd_body;

/// Resolve a 32-char base32 store-path hash to its `StorePath`.
fn nixhash(settings: &web::Data<Config>, hash: &[u8]) -> Result<Option<StorePath>> {
    // Validate the hash shape so garbage input becomes a 4xx, not a db scan.
    let store_path_hash =
        StorePathHash::decode_digest(hash).map_err(|e| StoreError::PathQuery {
            hash: String::from_utf8_lossy(hash).to_string(),
            reason: format!("Invalid hash format: {e}"),
        })?;

    settings
        .store
        .query_path_from_hash_part(&store_path_hash.to_string())
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
        // Exhaustive on `CacheError` so adding a variant forces an explicit
        // status decision rather than silently inheriting 500.
        match &self.err {
            CacheError::Store(StoreError::PathQuery { .. }) => StatusCode::NOT_FOUND,
            // Handlers wrap fs lookups in `io_context`; a missing file under a
            // resolved store path (GC race, stale client URL) is a lookup miss.
            CacheError::Io { source, .. } if source.kind() == std::io::ErrorKind::NotFound => {
                StatusCode::NOT_FOUND
            }
            CacheError::Io { .. }
            | CacheError::Store(StoreError::Db { .. })
            | CacheError::Config(_)
            | CacheError::Server(_)
            | CacheError::Signing(_)
            | CacheError::Fingerprint(_)
            | CacheError::NarInfo(_)
            // `ServeError::AccessDenied` is a server-side store anomaly (path
            // without a file name), not client authz, hence 500 not 403.
            | CacheError::Serve(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    // The default impl writes `Display` into the body, which would leak
    // filesystem paths and OS error strings embedded in `CacheError`. Log the
    // detail server-side and hand the client only the status phrase.
    fn error_response(&self) -> HttpResponse {
        let status = self.status_code();
        if status.is_server_error() {
            tracing::error!("request failed: {}", self.err);
        } else {
            tracing::debug!("request rejected: {}", self.err);
        }
        HttpResponse::build(status)
            .insert_header(cache_control_no_store())
            .content_type("text/plain; charset=utf-8")
            .body(status.canonical_reason().unwrap_or("error"))
    }
}

impl From<CacheError> for ServerError {
    fn from(err: CacheError) -> ServerError {
        ServerError { err }
    }
}

type ServerResult = std::result::Result<HttpResponse, ServerError>;

async fn inner_main() -> Result<()> {
    // Initialize tracing; bridges `log` records (actix, mio, rustls) via tracing-log.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let metrics = prometheus::initialize_metrics()?;
    let config = config::load()?;

    let c = web::Data::new(config);
    let config_data = c.clone();
    let metrics_data = web::Data::new(metrics.clone());

    let nar_route = format!("/nar/{{narhash:[{NIXBASE32_ALPHABET}]{{52}}}}.nar");
    // narinfos served by nix-serve have the narhash embedded in the nar URL.
    // While we don't do that, if nix-serve is replaced with harmonia, the old nar URLs
    // will stay in client caches for a while - so support them anyway.
    let nix_serve_nar_route = format!(
        "/nar/{{outhash:[{a}]{{32}}}}-{{narhash:[{a}]{{52}}}}.nar",
        a = NIXBASE32_ALPHABET
    );
    let mut server = HttpServer::new(move || {
        App::new()
            .wrap(middleware::Condition::new(
                config_data.enable_compression,
                zstd_body::ZstdMiddleware::new(config_data.zstd),
            ))
            .wrap(prometheus::PrometheusMiddleware::new(metrics.clone()))
            .app_data(config_data.clone())
            .app_data(metrics_data.clone())
            .route("/", web::get().to(root::get))
            .route("/{hash}.ls", web::get().to(narlist::get))
            .route("/{hash}.ls", web::head().to(narlist::get))
            .route("/{hash}.narinfo", web::get().to(narinfo::get))
            .route("/{hash}.narinfo", web::head().to(narinfo::get))
            .route(&nar_route, web::get().to(nar::get))
            .route(&nix_serve_nar_route, web::get().to(nar::get))
            .route("/serve/{hash}{path:.*}", web::get().to(serve::get))
            .route("/serve/{hash}{path:.*}", web::head().to(serve::get))
            .route("/log/{drv}", web::get().to(buildlog::get))
            .route("/version", web::get().to(version::get))
            .route("/health", web::get().to(health::get))
            .route("/nix-cache-info", web::get().to(cacheinfo::get))
            .route(
                "/build-trace-v2/{drv_path}/{output}",
                web::get().to(realisations::get),
            )
            .route("/metrics", web::get().to(prometheus::metrics_handler))
    })
    // default is 5 seconds, which is too small when doing mass requests on slow machines
    .client_request_timeout(Duration::from_secs(30))
    // Disable Nagle so the short trailing chunk at the end of each response is
    // sent immediately instead of waiting for a delayed ACK; on keep-alive
    // connections that wait would otherwise serialize onto every request.
    .tcp_nodelay(true)
    .workers(c.workers)
    .max_connection_rate(c.max_connection_rate);
    if c.max_connections > 0 {
        server = server.max_connections(c.max_connections);
    }

    let tls_config = if c.tls_cert_path.is_some() || c.tls_key_path.is_some() {
        Some(tls::load_tls_config(
            Path::new(
                &c.tls_cert_path
                    .clone()
                    .expect("tls certificate path must be set when tls is enabled"),
            ),
            Path::new(
                &c.tls_key_path
                    .clone()
                    .expect("tls key path must be set when tls is enabled"),
            ),
        )?)
    } else {
        None
    };

    // systemd socket activation takes precedence over `bind`.
    let mut activated = false;
    for fd in systemd::inherited_fds() {
        match systemd::classify(fd)? {
            systemd::Listener::Tcp(tcp) => {
                tracing::info!("listening on inherited fd {} ({:?})", fd, tcp.local_addr());
                server = match tls_config.clone() {
                    Some(cfg) => server
                        .listen_rustls_0_23(tcp, cfg)
                        .io_context("Failed to listen on inherited TCP listener with TLS")?,
                    None => server
                        .listen(tcp)
                        .io_context("Failed to listen on inherited TCP listener")?,
                };
            }
            systemd::Listener::Unix(uds) => {
                if tls_config.is_some() {
                    tracing::warn!(
                        "TLS configured but inherited socket is a Unix domain socket; serving plaintext on it"
                    );
                }
                tracing::info!("listening on inherited fd {} ({:?})", fd, uds.local_addr());
                server = server
                    .listen_uds(uds)
                    .io_context("Failed to listen on inherited Unix listener")?;
            }
        }
        activated = true;
    }

    if !activated {
        tracing::info!("listening on {}", c.bind);
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

        if let Some(cfg) = tls_config {
            if uds {
                tracing::error!("TLS is not supported with Unix domain sockets.");
                std::process::exit(1);
            }
            server = server
                .bind_rustls_0_23(c.bind.clone(), cfg)
                .io_context("Failed to bind with TLS")?;
        } else if uds {
            if !cfg!(unix) {
                tracing::error!("Binding to Unix domain sockets is only supported on Unix.");
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
    }

    server.run().await.io_context("Failed to start server")
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    inner_main().await.map_err(std::io::Error::other)
}
