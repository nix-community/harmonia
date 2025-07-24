#![warn(clippy::dbg_macro)]

use actix_web::middleware;
use anyhow::bail;
use anyhow::Context;
use anyhow::Result;
use config::Config;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::{fmt::Display, time::Duration};
use url::Url;

use actix_web::{http, web, App, HttpResponse, HttpServer};
use harmonia_store_remote::protocol::StorePath;
use openssl::ssl::{SslAcceptor, SslFiletype, SslMethod};

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
mod health;
mod nar;
mod narinfo;
mod narlist;
mod root;
mod serve;
mod signing;
mod store;
mod version;

async fn nixhash(settings: &web::Data<Config>, hash: &[u8]) -> Result<Option<StorePath>> {
    if hash.len() != 32 {
        bail!("Hash is too short");
    }

    let mut daemon_guard = settings.store.get_daemon().await?;
    let daemon = daemon_guard.as_mut().unwrap();

    daemon
        .query_path_from_hash_part(hash)
        .await
        .context("Failed to query path")
}

const BOOTSTRAP_SOURCE: &str = r#"
  <link href="https://cdn.jsdelivr.net/npm/bootstrap@5.2.3/dist/css/bootstrap.min.css"
        rel="stylesheet"
        integrity="sha384-rbsA2VBKQhggwzxH7pPCaAqO46MgnOM80zW1RWuH61DGLwZJEdK2Kadq2F9CUG65"
         crossorigin="anonymous">
  <script src="https://cdn.jsdelivr.net/npm/bootstrap@5.2.3/dist/js/bootstrap.bundle.min.js"
          integrity="sha384-kenU1KFdBIe4zVF0s0G1M5b4hcpxyD9F7jL+jjXkk+Q2h455rYXK/7HAuoJl+0I4"
          crossorigin="anonymous"></script>
"#;

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
    err: anyhow::Error,
}

impl Display for ServerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "{}", self.err)?;
        for cause in self.err.chain().skip(1) {
            writeln!(f, "because: {}", cause)?;
        }
        Ok(())
    }
}

impl actix_web::error::ResponseError for ServerError {}

impl From<anyhow::Error> for ServerError {
    fn from(err: anyhow::Error) -> ServerError {
        ServerError { err }
    }
}

type ServerResult = Result<HttpResponse, ServerError>;

async fn inner_main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let c = web::Data::new(config::load().with_context(|| "Failed to load configuration")?);
    let config_data = c.clone();

    log::info!("listening on {}", c.bind);
    let mut server = 
        
        HttpServer::new(move || {
        App::new()
            .wrap(middleware::Compress::default())
            .app_data(config_data.clone())
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
    })
    // default is 5 seconds, which is too small when doing mass requests on slow machines
    .client_request_timeout(Duration::from_secs(30))
    .workers(c.workers)
    .max_connection_rate(c.max_connection_rate);

    let try_url = Url::parse(&c.bind);
    let (bind, uds) = {
        if try_url.is_ok() {
            let url = try_url.as_ref().unwrap();
            if url.scheme() != "unix" {
                (c.bind.as_str(), false)
            } else if url.host().is_none() {
                (url.path(), true)
            } else {
                bail!("Can only bind to file URLs without host portion.");
            }
        } else {
            (c.bind.as_str(), false)
        }
    };

    if c.tls_cert_path.is_some() || c.tls_key_path.is_some() {
        if uds {
            log::error!("TLS is not supported with Unix domain sockets.");
            std::process::exit(1);
        }
        let mut builder = SslAcceptor::mozilla_intermediate(SslMethod::tls())?;
        builder.set_private_key_file(c.tls_key_path.clone().unwrap(), SslFiletype::PEM)?;
        builder.set_certificate_chain_file(c.tls_cert_path.clone().unwrap())?;
        server = server.bind_openssl(c.bind.clone(), builder)?;
    } else if uds {
        if !cfg!(unix) {
            log::error!("Binding to Unix domain sockets is only supported on Unix.");
            std::process::exit(1);
        } else {
            let socket_path = Path::new(bind);
            server = server.bind_uds(socket_path)?;
            fs::set_permissions(socket_path, fs::Permissions::from_mode(0o777))?;
        }
    } else {
        server = server.bind(c.bind.clone())?;
    }

    server.run().await.context("Failed to start server")
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    inner_main().await.map_err(std::io::Error::other)
}
