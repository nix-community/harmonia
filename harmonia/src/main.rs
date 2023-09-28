use std::{fmt::Display, path::Path, time::Duration};
use url::Url;

use actix_web::{http, web, App, HttpResponse, HttpServer};

mod buildlog;
mod cacheinfo;
mod config;
mod health;
mod nar;
mod narinfo;
mod narlist;
mod root;
mod serve;
mod version;

fn nixhash(hash: &str) -> Option<String> {
    if hash.len() != 32 {
        return None;
    }
    libnixstore::query_path_from_hash_part(hash)
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

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    libnixstore::init();

    let c = match config::load() {
        Ok(v) => web::Data::new(v),
        Err(e) => {
            log::error!("{e}");
            e.chain()
                .skip(1)
                .for_each(|cause| log::error!("because: {}", cause));
            std::process::exit(1);
        }
    };
    let config_data = c.clone();

    log::info!("listening on {}", c.bind);
    let mut server = HttpServer::new(move || {
        App::new()
            .app_data(config_data.clone())
            .route("/", web::get().to(root::get))
            .route("/{hash}.ls", web::get().to(narlist::get))
            .route("/{hash}.ls", web::head().to(narlist::get))
            .route("/{hash}.narinfo", web::get().to(narinfo::get))
            .route("/{hash}.narinfo", web::head().to(narinfo::get))
            .route("/nar/{hash}.nar", web::get().to(nar::get))
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
            if url.scheme() != "file" {
                (c.bind.as_str(), false)
            } else if url.host().is_none() {
                (url.path(), true)
            } else {
                log::error!("Can only bind to file URLs without host portion.");
                std::process::exit(1)
            }
        } else {
            (c.bind.as_str(), false)
        }
    };

    server = if uds {
        if !cfg!(unix) {
            log::error!("Binding to Unix domain sockets is only supported on Unix.");
            std::process::exit(1);
        } else {
            server.bind_uds(Path::new(bind))?
        }
    } else {
        server.bind(bind)?
    };

    server.run().await
}
