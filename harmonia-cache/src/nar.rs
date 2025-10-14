use std::collections::BTreeMap;
use std::mem::size_of;

use crate::error::{CacheError, IoErrorContext, NarError, Result, StoreError};
use actix_web::web::Bytes;
use actix_web::{HttpRequest, HttpResponse, http, web};
use serde::Deserialize;
use std::fs::{self, Metadata};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use sync::mpsc::Sender;
use tokio::fs::File;
use tokio::io::AsyncReadExt;

use crate::config::Config;
use crate::{cache_control_max_age_1y, some_or_404};
use std::ffi::{OsStr, OsString};
use tokio::{sync, task};

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

// We send this error across thread boundaries, so it must be Send + Sync
#[derive(Debug)]
enum ThreadSafeError {}
impl std::error::Error for ThreadSafeError {}
impl std::fmt::Display for ThreadSafeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "error")
    }
}

fn alignment(size: u64) -> usize {
    let align = 8 - (size % 8);
    if align == 8 { 0 } else { align as usize }
}

async fn write_byte_slices(
    tx: &Sender<std::result::Result<Bytes, ThreadSafeError>>,
    slices: &[&[u8]],
) -> Result<()> {
    let total_len = slices
        .iter()
        .map(|slice| size_of::<u64>() + slice.len() + alignment(slice.len() as u64))
        .sum();

    let mut vec = Vec::with_capacity(total_len);
    for slice in slices {
        vec.extend_from_slice(&(slice.len() as u64).to_le_bytes());
        vec.extend_from_slice(slice);
        vec.extend_from_slice(&[0u8; 8][0..alignment(slice.len() as u64)]);
    }

    tx.send(Ok(Bytes::from(vec)))
        .await
        .map_err(|e| NarError::ChannelSend {
            reason: format!("Failed to send byte slices: {e}"),
        })?;
    Ok(())
}

async fn dump_contents(
    p: &Path,
    expected_size: u64,
    tx: &Sender<std::result::Result<Bytes, ThreadSafeError>>,
) -> Result<()> {
    let mut file = File::open(p).await.map_err(|e| {
        log::warn!("Failed to open file for dumping contents: {}", p.display());
        NarError::ReadFile {
            path: p.display().to_string(),
            source: e,
        }
    })?;
    let mut left = expected_size;

    loop {
        let mut buf = vec![0; 16384];

        let n = file.read(&mut buf).await.io_context(format!(
            "Failed to read file for dumping contents: {}",
            p.display()
        ))?;
        if n == 0 {
            if left != 0 {
                log::warn!(
                    "Read less bytes than expected while dumping contents: {}",
                    p.to_string_lossy()
                );
                return Err(NarError::Streaming {
                    reason: format!(
                        "Unexpected end of file while dumping contents: {}",
                        p.display()
                    ),
                }
                .into());
            }
            // add zero padding at the end
            buf.resize(n + alignment(expected_size), 0);
            tx.send(Ok(Bytes::from(buf)))
                .await
                .map_err(|e| NarError::ChannelSend {
                    reason: format!("Failed to send final NAR chunk with padding: {e}"),
                })?;
            break;
        }
        if n as u64 > left {
            log::warn!(
                "Read more bytes than expected while dumping contents: {}",
                p.to_string_lossy()
            );
            return Err(NarError::Streaming {
                reason: format!(
                    "Read more bytes than expected while dumping contents: {}",
                    p.display()
                ),
            }
            .into());
        }
        left -= n as u64;

        tx.send(Ok(Bytes::from(buf).slice(0..n)))
            .await
            .map_err(|e| NarError::ChannelSend {
                reason: format!("Failed to send NAR chunk: {e}"),
            })?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn strip_case_hack_suffix(s: &OsStr) -> &OsStr {
    let needle = b"~nix~case~hack~";
    let pos = s
        .as_bytes()
        .windows(needle.len())
        .position(|window| window == needle);
    if let Some(pos) = pos {
        OsStr::from_bytes(&s.as_bytes()[0..pos])
    } else {
        s
    }
}

#[cfg(not(target_os = "macos"))]
fn strip_case_hack_suffix(s: &OsStr) -> &OsStr {
    s
}

struct Frame {
    path: PathBuf,
    metadata: Metadata,
    children: Option<BTreeMap<OsString, OsString>>,
    first_child: bool,
}

impl Frame {
    async fn new(path: PathBuf) -> Result<Self> {
        let metadata = tokio::fs::symlink_metadata(&path)
            .await
            .io_context(format!(
                "Failed to get metadata for path: {}",
                path.display()
            ))?;
        let children = if metadata.is_dir() {
            let mut read_dir = tokio::fs::read_dir(&path).await.io_context(format!(
                "Failed to read directory for path: {}",
                path.display()
            ))?;
            let mut entries = BTreeMap::new();
            while let Some(e) = read_dir
                .next_entry()
                .await
                .io_context("Failed to read directory")?
            {
                let file_name = e.file_name();
                if file_name == "." || file_name == ".." {
                    continue;
                }
                entries.insert(strip_case_hack_suffix(&file_name).to_owned(), file_name);
            }
            if entries.is_empty() {
                None
            } else {
                Some(entries)
            }
        } else {
            None
        };

        Ok(Self {
            path,
            metadata,
            children,
            first_child: true,
        })
    }
}

async fn dump_file(
    frame: &Frame,
    tx: &Sender<std::result::Result<Bytes, ThreadSafeError>>,
) -> Result<()> {
    if frame.metadata.permissions().mode() & 0o100 != 0 {
        write_byte_slices(
            tx,
            &[b"(", b"type", b"regular", b"executable", b"", b"contents"],
        )
        .await?;
    } else {
        write_byte_slices(tx, &[b"(", b"type", b"regular", b"contents"]).await?;
    }
    tx.send(Ok(Bytes::from(frame.metadata.len().to_le_bytes().to_vec())))
        .await
        .map_err(|e| NarError::ChannelSend {
            reason: format!("Failed to send file size: {e}"),
        })?;

    dump_contents(&frame.path, frame.metadata.len(), tx).await?;
    write_byte_slices(tx, &[b")"]).await?;
    Ok(())
}

async fn dump_symlink(
    frame: &Frame,
    tx: &Sender<std::result::Result<Bytes, ThreadSafeError>>,
) -> Result<()> {
    let link_target = fs::read_link(&frame.path).map_err(|e| NarError::SymlinkRead {
        path: frame.path.display().to_string(),
        source: e,
    })?;
    write_byte_slices(
        tx,
        &[
            b"(",
            b"type",
            b"symlink",
            b"target",
            link_target.as_os_str().as_bytes(),
            b")",
        ],
    )
    .await?;
    Ok(())
}

async fn dump_path(
    path: PathBuf,
    tx: &Sender<std::result::Result<Bytes, ThreadSafeError>>,
) -> Result<()> {
    write_byte_slices(tx, &[b"nix-archive-1"]).await?;
    let mut stack = vec![Frame::new(path).await?];

    while let Some(frame) = stack.last_mut() {
        let file_type = frame.metadata.file_type();
        if file_type.is_dir() {
            if frame.first_child {
                write_byte_slices(tx, &[b"(", b"type", b"directory"]).await?;
                if frame.children.is_none() {
                    // end directory
                    write_byte_slices(tx, &[b")"]).await?;
                    // pop directory from stack
                    stack.pop();
                    continue;
                }
            }

            if let Some(childrens) = frame.children.as_mut() {
                if frame.first_child {
                    frame.first_child = false;
                } else {
                    // end entry
                    write_byte_slices(tx, &[b")"]).await?;
                }
                if let Some((nar_name, name)) = childrens.pop_first() {
                    write_byte_slices(tx, &[b"entry", b"(", b"name", nar_name.as_bytes(), b"node"])
                        .await?;
                    let path = frame.path.join(name);
                    stack.push(Frame::new(path).await?);
                } else {
                    // end directory
                    write_byte_slices(tx, &[b")"]).await?;
                    // pop directory from stack
                    stack.pop();
                }
            }
        } else {
            if file_type.is_file() {
                dump_file(frame, tx).await?;
            } else if file_type.is_symlink() {
                dump_symlink(frame, tx).await?;
            } else {
                return Err(NarError::Streaming {
                    reason: format!("Unsupported file type for path: {}", frame.path.display()),
                }
                .into());
            }
            stack.pop();
        }
    }

    Ok(())
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
            let mut daemon_guard = settings.store.get_daemon().await.map_err(|e| {
                CacheError::from(StoreError::Operation {
                    reason: format!("Failed to get daemon connection: {e}"),
                })
            })?;
            let daemon = daemon_guard.as_mut().unwrap();
            daemon
                .query_path_from_hash_part(outhash.as_bytes())
                .await
                .map_err(|e| {
                    CacheError::from(StoreError::PathQuery {
                        hash: String::from_utf8_lossy(outhash.as_bytes()).to_string(),
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
        let mut daemon_guard = settings.store.get_daemon().await.map_err(|e| {
            CacheError::from(StoreError::Operation {
                reason: format!("Failed to get daemon connection: {e}"),
            })
        })?;
        let daemon = daemon_guard.as_mut().unwrap();

        match daemon
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
    }; // daemon_guard is dropped here

    if narhash.as_bytes() != info.hash.to_nix_base32() {
        return Ok(HttpResponse::NotFound()
            .insert_header(crate::cache_control_no_store())
            .body("hash mismatch detected"));
    }

    let mut rlength = info.nar_size;
    let offset;
    let mut res = HttpResponse::Ok();

    let (tx, rx) = tokio::sync::mpsc::channel::<std::result::Result<Bytes, ThreadSafeError>>(1000);
    let rx = tokio_stream::wrappers::ReceiverStream::new(rx);

    // Credit actix_web actix-files: https://github.com/actix/actix-web/blob/master/actix-files/src/named.rs#L525
    if let Some(ranges) = req.headers().get(http::header::RANGE) {
        if let Ok(ranges_header) = ranges.to_str() {
            if let Ok(ranges) = HttpRange::parse(ranges_header, rlength) {
                rlength = ranges[0].length;
                offset = ranges[0].start;

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
                        offset + rlength - 1,
                        info.nar_size
                    ),
                ));
            } else {
                res.insert_header((http::header::CONTENT_RANGE, format!("bytes */{rlength}")));
                return Ok(res.status(http::StatusCode::RANGE_NOT_SATISFIABLE).finish());
            };
        } else {
            return Ok(res.status(http::StatusCode::BAD_REQUEST).finish());
        };
        let mut send: u64 = 0;

        let (tx2, mut rx2) =
            tokio::sync::mpsc::channel::<std::result::Result<Bytes, ThreadSafeError>>(1000);
        task::spawn(async move {
            // If Nix is set to a non-root store, physical store paths will differ from
            // logical paths. Below we check if that is the case, and rewrite to physical
            // before dumping.

            let err = dump_path(settings.store.get_real_path(&store_path), &tx2).await;
            if let Err(err) = err {
                log::error!("Error dumping path {store_path}: {err}");
            }
        });
        // we keep this closure extra to avoid unaligned copies in the non-range request case.
        task::spawn(async move {
            while let Some(Ok(data)) = rx2.recv().await {
                let len = data.len() as u64;
                if send + len > offset {
                    let start = offset.saturating_sub(send);
                    let end = if send + data.len() as u64 > offset + rlength {
                        start + rlength
                    } else {
                        len
                    };
                    // does it fit into usize
                    let start: usize = match start.try_into() {
                        Ok(v) => v,
                        Err(e) => {
                            log::error!("BUG: start(u64) is too big for usize: {e}");
                            break;
                        }
                    };
                    let end: usize = match end.try_into() {
                        Ok(v) => v,
                        Err(e) => {
                            log::error!("BUG: end(u64) is too big for usize: {e}");
                            break;
                        }
                    };
                    if tx.send(Ok(data.slice(start..end))).await.is_err() {
                        break;
                    }
                }
                send += len;
            }
        });
    } else {
        task::spawn(async move {
            let err = dump_path(settings.store.get_real_path(&store_path), &tx).await;
            if let Err(err) = err {
                log::error!("Error dumping path {store_path}: {err}");
            }
        });
    };

    Ok(res
        .insert_header((http::header::CONTENT_TYPE, "application/x-nix-archive"))
        .insert_header((http::header::ACCEPT_RANGES, "bytes"))
        .insert_header(cache_control_max_age_1y())
        .body(actix_web::body::SizedStream::new(rlength, rx)))
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::store::Store;
    use harmonia_store_remote::protocol::StorePath;
    use std::process::Command;

    async fn dump_to_vec(path: String) -> Result<Vec<u8>> {
        let store = Store::new(
            b"/nix/store".to_vec(),
            None,
            std::path::PathBuf::from("/nix/var/nix/daemon-socket/socket"),
            harmonia_store_remote::client::PoolConfig {
                max_size: 2, // Small pool for tests
                ..Default::default()
            },
        );
        let (tx, mut rx) =
            tokio::sync::mpsc::channel::<std::result::Result<Bytes, ThreadSafeError>>(1000);
        task::spawn(async move {
            let store_path = StorePath::from(path.into_bytes());
            let e = dump_path(store.get_real_path(&store_path), &tx).await;
            if let Err(e) = e {
                eprintln!("Error dumping path: {e}");
            }
        });
        let mut resp = Vec::new();
        let mut i = 0;
        loop {
            match rx.recv().await {
                Some(Ok(bytes)) => {
                    resp.extend_from_slice(&bytes);
                }
                Some(Err(e)) => {
                    return Err(NarError::Streaming {
                        reason: format!("Got error: {e}"),
                    }
                    .into());
                }
                None => {
                    if i > 100 {
                        break;
                    }
                    i += 1;
                }
            }
        }
        Ok(resp)
    }
    // Useful for debugging
    fn pretty_hex_dump(bytes: &[u8]) {
        let mut i = 0;
        while i < bytes.len() {
            let mut line = String::new();
            for j in 0..16 {
                if i + j < bytes.len() {
                    line.push_str(&format!("{:02x} ", bytes[i + j]));
                } else {
                    line.push_str("   ");
                }
            }
            line.push_str(" | ");
            for j in 0..16 {
                if i + j < bytes.len() {
                    if bytes[i + j] >= 32 && bytes[i + j] < 127 {
                        line.push(bytes[i + j] as char);
                    } else {
                        line.push('.');
                    }
                } else {
                    line.push(' ');
                }
            }
            println!("{line}");
            i += 16;
        }
    }

    #[tokio::test]
    async fn test_dump_store() -> Result<()> {
        let temp_dir = tempfile::tempdir()
            .map_err(|e| NarError::Streaming {
                reason: format!("Failed to create temp dir: {e}"),
            })
            .expect("Failed to create temp dir");
        let dir = temp_dir.path();
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

        let nar_dump = dump_to_vec(dir.to_str().unwrap().to_owned()).await?;
        let res = Command::new("nix-store")
            .arg("--dump")
            .arg(dir)
            .output()
            .map_err(|e| NarError::Streaming {
                reason: format!("Failed to run nix-store --dump: {e}"),
            })?;
        assert_eq!(res.status.code(), Some(0));
        println!("nar_dump:");
        pretty_hex_dump(&nar_dump);
        println!("nix-store --dump:");
        pretty_hex_dump(&res.stdout);
        assert_eq!(res.stdout, nar_dump);

        Ok(())
    }
}
