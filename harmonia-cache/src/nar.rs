use crate::config::Config;
use crate::error::{CacheError, StoreError};
use crate::{cache_control_max_age_1y, some_or_404};
use actix_web::web::Bytes;
use actix_web::{HttpRequest, HttpResponse, http, web};
use harmonia_nar::NarByteStream;
use harmonia_store_core::store_path::StorePathHash;
use harmonia_store_remote::DaemonStore;
use harmonia_utils_hash::fmt::CommonHash;
use serde::Deserialize;

/// Represents the query string of a NAR URL.
#[derive(Debug, Deserialize)]
pub struct NarRequest {
    hash: Option<String>,
}

/// Represents the parsed parts in a NAR URL.
#[derive(Debug, Deserialize)]
pub struct PathParams {
    narhash: String,
    outhash: Option<String>,
}

// TODO(conni2461): still missing
// - handle downloadHash/downloadSize and fileHash/fileSize after implementing compression

// Credit actix_web actix-files: https://github.com/actix/actix-web/blob/master/actix-files/src/range.rs
#[derive(Debug)]
struct HttpRange {
    start: u64,
    length: u64,
}

impl HttpRange {
    /// Parses Range HTTP header string as per RFC 2616.
    ///
    /// `header` is HTTP Range header (e.g. `bytes=bytes=0-9`).
    /// `size` is full size of response (file).
    fn parse(
        header: &str,
        size: u64,
    ) -> std::result::Result<Vec<Self>, http_range::HttpRangeParseError> {
        http_range::HttpRange::parse(header, size).map(|ranges| {
            ranges
                .iter()
                .map(|range| Self {
                    start: range.start,
                    length: range.length,
                })
                .collect()
        })
    }
}

pub(crate) async fn get(
    path: web::Path<PathParams>,
    req: HttpRequest,
    q: web::Query<NarRequest>,
    settings: web::Data<Config>,
) -> crate::ServerResult {
    // Extract the narhash from the query parameter, and bail out if it's missing or invalid.
    let narhash = some_or_404!(Some(path.narhash.as_str()));

    // lookup the store path.
    // We usually extract the outhash from the query parameter.
    // However, when processing nix-serve URLs, it's present in the path
    // directly.
    let outhash = if let Some(outhash) = &q.hash {
        Some(outhash.as_str())
    } else {
        path.outhash.as_deref()
    };
    let store_path = match outhash {
        Some(outhash) => {
            // Parse outhash to StorePathHash
            let store_path_hash =
                StorePathHash::decode_digest(outhash.as_bytes()).map_err(|e| {
                    CacheError::from(StoreError::PathQuery {
                        hash: outhash.to_string(),
                        reason: format!("Invalid hash format: {e}"),
                    })
                })?;

            let mut guard = settings.store.acquire().await?;
            guard
                .client()
                .query_path_from_hash_part(&store_path_hash)
                .await
                .map_err(|e| {
                    CacheError::from(StoreError::PathQuery {
                        hash: outhash.to_string(),
                        reason: e.to_string(),
                    })
                })?
        }
        None => {
            return Ok(HttpResponse::NotFound()
                .insert_header(crate::cache_control_no_store())
                .body("missing outhash"));
        }
    };
    let store_path = match store_path {
        Some(store_path) => store_path,
        None => {
            return Ok(HttpResponse::NotFound()
                .insert_header(crate::cache_control_no_store())
                .body("store path not found"));
        }
    };

    // lookup the path info.
    let info = {
        let mut guard = settings.store.acquire().await?;

        match guard
            .client()
            .query_path_info(&store_path)
            .await
            .map_err(|e| CacheError::from(StoreError::Remote(e)))?
        {
            Some(info) => info,
            None => {
                return Ok(HttpResponse::NotFound()
                    .insert_header(crate::cache_control_no_store())
                    .body("path info not found"));
            }
        }
    }; // guard is dropped here

    // URL narhash is bare (no sha256: prefix), so use as_bare() for comparison
    let expected_hash = info.nar_hash.as_base32().as_bare().to_string();
    if narhash != expected_hash {
        return Ok(HttpResponse::NotFound()
            .insert_header(crate::cache_control_no_store())
            .body("hash mismatch detected"));
    }

    let rlength = info.nar_size;
    let mut res = HttpResponse::Ok();

    let real_path = settings.store.get_real_path(&store_path);

    // Credit actix_web actix-files: https://github.com/actix/actix-web/blob/master/actix-files/src/named.rs#L525
    if let Some(ranges) = req.headers().get(http::header::RANGE) {
        if let Ok(ranges_header) = ranges.to_str() {
            if let Ok(ranges) = HttpRange::parse(ranges_header, rlength) {
                let range_length = ranges[0].length;
                let offset = ranges[0].start;

                if settings.enable_compression {
                    // don't allow compression middleware to modify partial content
                    res.insert_header((
                        http::header::CONTENT_ENCODING,
                        http::header::HeaderValue::from_static("none"),
                    ));
                }

                res.insert_header((
                    http::header::CONTENT_RANGE,
                    format!(
                        "bytes {}-{}/{}",
                        offset,
                        offset + range_length - 1,
                        info.nar_size
                    ),
                ));

                // For range requests, we need to skip bytes and limit output
                let stream = NarByteStream::new(real_path);
                let ranged_stream = create_range_stream(stream, offset, range_length);

                return Ok(res
                    .insert_header((http::header::CONTENT_TYPE, "application/x-nix-archive"))
                    .insert_header((http::header::ACCEPT_RANGES, "bytes"))
                    .insert_header(cache_control_max_age_1y())
                    .body(actix_web::body::SizedStream::new(
                        range_length,
                        ranged_stream,
                    )));
            } else {
                res.insert_header((http::header::CONTENT_RANGE, format!("bytes */{rlength}")));
                return Ok(res.status(http::StatusCode::RANGE_NOT_SATISFIABLE).finish());
            };
        } else {
            return Ok(res.status(http::StatusCode::BAD_REQUEST).finish());
        };
    }

    // Non-range request: stream the full NAR
    let stream = NarByteStream::new(real_path);

    Ok(res
        .insert_header((http::header::CONTENT_TYPE, "application/x-nix-archive"))
        .insert_header((http::header::ACCEPT_RANGES, "bytes"))
        .insert_header(cache_control_max_age_1y())
        .body(actix_web::body::SizedStream::new(rlength, stream)))
}

/// Create a stream that skips `offset` bytes and returns at most `length` bytes.
fn create_range_stream<S>(
    stream: S,
    offset: u64,
    length: u64,
) -> impl futures::Stream<Item = std::result::Result<Bytes, std::io::Error>>
where
    S: futures::Stream<Item = std::result::Result<Bytes, std::io::Error>> + Unpin,
{
    futures::stream::unfold(
        (stream, offset, length, 0u64),
        |(mut stream, offset, length, mut sent)| async move {
            use futures::StreamExt;

            loop {
                match stream.next().await {
                    Some(Ok(data)) => {
                        let data_len = data.len() as u64;

                        // If we haven't reached the offset yet
                        if sent + data_len <= offset {
                            sent += data_len;
                            continue;
                        }

                        // Calculate the slice we need from this chunk
                        let start = if sent < offset {
                            (offset - sent) as usize
                        } else {
                            0
                        };

                        let remaining = length - (sent.saturating_sub(offset).min(length));
                        if remaining == 0 {
                            return None;
                        }

                        let end = (start as u64 + remaining).min(data_len) as usize;

                        sent += data_len;

                        if start < end {
                            let slice = data.slice(start..end);
                            return Some((Ok(slice), (stream, offset, length, sent)));
                        }
                    }
                    Some(Err(e)) => return Some((Err(e), (stream, offset, length, sent))),
                    None => return None,
                }
            }
        },
    )
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::error::{IoErrorContext, Result};
    use futures::StreamExt;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::process::Command;

    async fn dump_to_vec(path: PathBuf) -> Vec<u8> {
        let stream = NarByteStream::new(path);
        futures::pin_mut!(stream);

        let mut result = Vec::new();
        while let Some(chunk) = stream.next().await {
            let bytes = chunk.expect("Stream error during NAR dump");
            result.extend_from_slice(&bytes);
        }
        result
    }

    #[tokio::test]
    async fn test_dump_store() -> Result<()> {
        let temp_dir =
            harmonia_utils_test::CanonicalTempDir::new().expect("Failed to create temp dir");
        let dir = temp_dir.path().to_path_buf();
        fs::write(dir.join("file"), b"somecontent").io_context("Failed to write test file")?;

        fs::create_dir(dir.join("some_empty_dir")).io_context("Failed to create test empty dir")?;

        let some_dir = dir.join("some_dir");
        fs::create_dir(&some_dir).io_context("Failed to create test dir")?;

        let executable_path = some_dir.join("executable");
        fs::write(&executable_path, b"somescript").io_context("Failed to write test executable")?;
        fs::set_permissions(&executable_path, fs::Permissions::from_mode(0o755))
            .io_context("Failed to set test executable permissions")?;

        std::os::unix::fs::symlink("sometarget", dir.join("symlink"))
            .io_context("Failed to create test symlink")?;

        let nar_dump = dump_to_vec(dir.clone()).await;
        let res = Command::new("nix-store")
            .arg("--dump")
            .arg(dir)
            .output()
            .expect("Failed to run nix-store --dump");
        assert_eq!(res.status.code(), Some(0));
        println!("nar_dump len: {}", nar_dump.len());
        println!("nix-store --dump len: {}", res.stdout.len());
        // println!("nix-store --dump:");
        assert_eq!(res.stdout, nar_dump);

        Ok(())
    }
}
