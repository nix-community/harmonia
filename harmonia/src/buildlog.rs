use actix_files::NamedFile;
use actix_web::http::header::HeaderValue;
use actix_web::Responder;
use actix_web::{http, web, HttpRequest, HttpResponse};
use anyhow::Context;
use async_compression::tokio::bufread::BzDecoder;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::path::PathBuf;
use tokio::io::BufReader;
use tokio_util::io::ReaderStream;

use crate::config::Config;
use crate::{cache_control_max_age_1y, cache_control_no_store, nixhash, some_or_404};

fn query_drv_path(drv: &str) -> Option<String> {
    nixhash(if drv.len() > 32 { &drv[0..32] } else { drv })
}

pub fn get_build_log(store: &Path, drv_path: &Path) -> Option<PathBuf> {
    let drv_name = drv_path.file_name()?.as_bytes();
    let log_path = match store.parent().map(|p| {
        p.join("var")
            .join("log")
            .join("nix")
            .join("drvs")
            .join(OsStr::from_bytes(&drv_name[0..2]))
            .join(OsStr::from_bytes(&drv_name[2..]))
    }) {
        Some(log_path) => log_path,
        None => return None,
    };
    if log_path.exists() {
        return Some(log_path);
    }
    // check if compressed log exists
    let log_path = log_path.with_extension("drv.bz2");
    if log_path.exists() {
        Some(log_path)
    } else {
        None
    }
}

pub(crate) async fn get(
    drv: web::Path<String>,
    req: HttpRequest,
    settings: web::Data<Config>,
) -> Result<HttpResponse, Box<dyn std::error::Error>> {
    let drv_path = some_or_404!(query_drv_path(&drv));
    if libnixstore::is_valid_path(&drv_path) {
        let build_log = some_or_404!(get_build_log(
            settings.store.real_store(),
            &PathBuf::from(drv_path)
        ));
        if let Some(ext) = build_log.extension() {
            let accept_encoding = req
                .headers()
                .get(http::header::ACCEPT_ENCODING)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("");

            if ext == "bz2" && !accept_encoding.contains("bzip2") {
                // Decompress the bz2 file and serve the decompressed content
                let file = tokio::fs::File::open(&build_log).await.with_context(|| {
                    format!("Failed to open build log: {:?}", build_log.display())
                })?;
                let reader = BufReader::new(file);
                let decompressed_stream = BzDecoder::new(reader);
                let stream = ReaderStream::new(decompressed_stream);
                let body = actix_web::body::BodyStream::new(stream);

                return Ok(HttpResponse::Ok()
                    .insert_header(cache_control_max_age_1y())
                    .insert_header(http::header::ContentType(mime::TEXT_PLAIN_UTF_8))
                    .body(body));
            } else {
                // Serve the file as-is with the appropriate Content-Encoding header
                let encoding = if ext == "bz2" {
                    HeaderValue::from_static("bzip2")
                } else {
                    HeaderValue::from_static("identity")
                };

                let log = NamedFile::open_async(&build_log)
                    .await
                    .with_context(|| {
                        format!("Failed to open build log: {:?}", build_log.display())
                    })?
                    .customize()
                    .insert_header(cache_control_max_age_1y())
                    .insert_header(("Content-Encoding", encoding));

                return Ok(log.respond_to(&req).map_into_boxed_body());
            }
        }
    }
    Ok(HttpResponse::NotFound()
        .insert_header(cache_control_no_store())
        .finish())
}
