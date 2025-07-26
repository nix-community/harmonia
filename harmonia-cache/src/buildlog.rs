use crate::error::{BuildLogError, CacheError, IoErrorContext, Result, StoreError};
use actix_files::NamedFile;
use actix_web::Responder;
use actix_web::http::header::HeaderValue;
use actix_web::{HttpRequest, HttpResponse, http, web};
use async_compression::tokio::bufread::BzDecoder;
use harmonia_store_remote::protocol::StorePath;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::path::PathBuf;
use tokio::io::BufReader;
use tokio_util::io::ReaderStream;

use crate::config::Config;
use crate::{cache_control_max_age_1y, cache_control_no_store, nixhash, some_or_404};

async fn query_drv_path(settings: &web::Data<Config>, drv: &[u8]) -> Result<Option<StorePath>> {
    nixhash(settings, if drv.len() > 32 { &drv[0..32] } else { drv }).await
}

pub fn get_build_log(store: &Path, drv_path: &StorePath) -> Option<PathBuf> {
    let drv_path = Path::new(std::ffi::OsStr::from_bytes(drv_path.as_bytes()));
    let drv_name = drv_path.file_name()?.as_bytes();
    let log_path = store.parent().map(|p| {
        p.join("var")
            .join("log")
            .join("nix")
            .join("drvs")
            .join(OsStr::from_bytes(&drv_name[0..2]))
            .join(OsStr::from_bytes(&drv_name[2..]))
    })?;
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
) -> crate::ServerResult {
    let drv_path = some_or_404!(
        query_drv_path(&settings, drv.as_bytes())
            .await
            .map_err(|e| CacheError::from(BuildLogError::QueryFailed {
                reason: format!("Could not query nar hash in database for {drv}: {e}"),
            }))?
    );
    let mut daemon_guard = settings.store.get_daemon().await.map_err(|e| {
        CacheError::from(StoreError::Operation {
            reason: format!("Failed to get daemon connection: {e}"),
        })
    })?;
    let daemon = daemon_guard.as_mut().unwrap();

    match daemon.is_valid_path(&drv_path).await {
        Ok(true) => (),
        Ok(false) => {
            return Ok(HttpResponse::NotFound()
                .insert_header(cache_control_no_store())
                .finish());
        }
        Err(e) => {
            return Ok(HttpResponse::InternalServerError()
                .insert_header(cache_control_no_store())
                .body(format!("Failed to query path info: {e}")));
        }
    }
    let build_log = some_or_404!(get_build_log(settings.store.real_store(), &drv_path));
    let ext = match build_log.extension() {
        Some(ext) => ext,
        None => {
            return Ok(HttpResponse::NotFound()
                .insert_header(cache_control_no_store())
                .finish());
        }
    };
    let accept_encoding = req
        .headers()
        .get(http::header::ACCEPT_ENCODING)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");

    if ext == "bz2" && !accept_encoding.contains("bzip2") {
        // Decompress the bz2 file and serve the decompressed content
        let file = tokio::fs::File::open(&build_log)
            .await
            .io_context(format!("Failed to open build log: {}", build_log.display()))?;
        let reader = BufReader::new(file);
        let decompressed_stream = BzDecoder::new(reader);
        let stream = ReaderStream::new(decompressed_stream);
        let body = actix_web::body::BodyStream::new(stream);

        return Ok(HttpResponse::Ok()
            .insert_header(cache_control_max_age_1y())
            .insert_header(http::header::ContentType(mime::TEXT_PLAIN_UTF_8))
            .body(body));
    }

    // Serve the file as-is with the appropriate Content-Encoding header
    let encoding = if ext == "bz2" {
        HeaderValue::from_static("bzip2")
    } else {
        HeaderValue::from_static("identity")
    };

    let log = NamedFile::open_async(&build_log)
        .await
        .io_context(format!("Failed to open build log: {}", build_log.display()))?
        .customize()
        .insert_header(cache_control_max_age_1y())
        .insert_header(("Content-Encoding", encoding));

    Ok(log.respond_to(&req).map_into_boxed_body())
}
