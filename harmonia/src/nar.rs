use std::collections::BTreeMap;
use std::error::Error;
use std::mem::size_of;

use actix_web::web::Bytes;
use actix_web::{http, web, HttpRequest, HttpResponse};
use anyhow::{bail, Context, Result};
use libnixstore::Radix;
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
    if align == 8 {
        0
    } else {
        align as usize
    }
}

async fn write_byte_slices(
    tx: &Sender<Result<Bytes, ThreadSafeError>>,
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
        .context("Failed to send")
}

async fn dump_contents(
    p: &Path,
    expected_size: u64,
    tx: &Sender<Result<Bytes, ThreadSafeError>>,
) -> Result<()> {
    let mut file = File::open(p).await.with_context(|| {
        log::warn!("Failed to open file for dumping contents: {}", p.display());
        format!(
            "Failed to open file for dumping contents: {}",
            p.to_string_lossy()
        )
    })?;
    let mut left = expected_size;

    loop {
        let mut buf = vec![0; 16384];

        let n = file.read(&mut buf).await.with_context(|| {
            format!(
                "Failed to read file for dumping contents: {}",
                p.to_string_lossy()
            )
        })?;
        if n == 0 {
            if left != 0 {
                log::warn!(
                    "Read less bytes than expected while dumping contents: {}",
                    p.to_string_lossy()
                );
                bail!(
                    "Unexpected end of file while dumping contents: {}",
                    p.to_string_lossy()
                )
            }
            // add zero padding at the end
            buf.resize(n + alignment(expected_size), 0);
            tx.send(Ok(Bytes::from(buf)))
                .await
                .context("Failed to send")?;
            break;
        }
        if n as u64 > left {
            log::warn!(
                "Read more bytes than expected while dumping contents: {}",
                p.to_string_lossy()
            );
            bail!(
                "Read more bytes than expected while dumping contents: {}",
                p.to_string_lossy()
            )
        }
        left -= n as u64;

        tx.send(Ok(Bytes::from(buf).slice(0..n)))
            .await
            .context("Failed to send")?;
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
            .with_context(|| format!("Failed to get metadata for path: {}", path.display()))?;
        let children = if metadata.is_dir() {
            let mut read_dir = tokio::fs::read_dir(&path).await.with_context(|| {
                format!("Failed to read directory for path: {}", path.display())
            })?;
            let mut entries = BTreeMap::new();
            while let Some(e) = read_dir
                .next_entry()
                .await
                .context("Failed to read directory")?
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

async fn dump_file(frame: &Frame, tx: &Sender<Result<Bytes, ThreadSafeError>>) -> Result<()> {
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
        .context("Failed to send")?;

    dump_contents(&frame.path, frame.metadata.len(), tx).await?;
    write_byte_slices(tx, &[b")"]).await?;
    Ok(())
}

async fn dump_symlink(frame: &Frame, tx: &Sender<Result<Bytes, ThreadSafeError>>) -> Result<()> {
    let link_target = fs::read_link(&frame.path).with_context(|| {
        format!(
            "Failed to read link target for path: {}",
            frame.path.display()
        )
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

async fn dump_path(path: PathBuf, tx: &Sender<Result<Bytes, ThreadSafeError>>) -> Result<()> {
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
                bail!("Unsupported file type: {:?}", file_type);
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
) -> Result<HttpResponse, Box<dyn Error>> {
    // Extract the narhash from the query parameter, and bail out if it's missing or invalid.
    let narhash = some_or_404!(Some(path.narhash.as_str()));

    // lookup the store path.
    let store_path = some_or_404!({
        // We usually extract the outhash from the query parameter.
        // However, when processing nix-serve URLs, it's present in the path
        // directly.
        if let Some(outhash) = &q.hash {
            Some(outhash.as_str())
        } else {
            path.outhash.as_deref()
        }
    }
    .and_then(libnixstore::query_path_from_hash_part));

    // lookup the path info.
    let info = libnixstore::query_path_info(&store_path, Radix::default())?;
    // ensure the narhash specified in the request matches.
    if format!("sha256:{}", narhash) != info.narhash {
        return Ok(HttpResponse::NotFound()
            .insert_header(crate::cache_control_no_store())
            .body("hash mismatch detected"));
    }
    let store_path = PathBuf::from(&store_path);

    let mut rlength = info.size;
    let offset;
    let mut res = HttpResponse::Ok();

    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, ThreadSafeError>>(1000);
    let rx = tokio_stream::wrappers::ReceiverStream::new(rx);

    // Credit actix_web actix-files: https://github.com/actix/actix-web/blob/master/actix-files/src/named.rs#L525
    if let Some(ranges) = req.headers().get(http::header::RANGE) {
        if let Ok(ranges_header) = ranges.to_str() {
            if let Ok(ranges) = HttpRange::parse(ranges_header, rlength) {
                rlength = ranges[0].length;
                offset = ranges[0].start;

                // don't allow compression middleware to modify partial content
                res.insert_header((
                    http::header::CONTENT_ENCODING,
                    http::header::HeaderValue::from_static("identity"),
                ));

                res.insert_header((
                    http::header::CONTENT_RANGE,
                    format!("bytes {}-{}/{}", offset, offset + rlength - 1, info.size),
                ));
            } else {
                res.insert_header((http::header::CONTENT_RANGE, format!("bytes */{}", rlength)));
                return Ok(res.status(http::StatusCode::RANGE_NOT_SATISFIABLE).finish());
            };
        } else {
            return Ok(res.status(http::StatusCode::BAD_REQUEST).finish());
        };
        let mut send: u64 = 0;

        let (tx2, mut rx2) = tokio::sync::mpsc::channel::<Result<Bytes, ThreadSafeError>>(1000);
        task::spawn(async move {
            // If Nix is set to a non-root store, physical store paths will differ from
            // logical paths. Below we check if that is the case, and rewrite to physical
            // before dumping.

            let err = dump_path(settings.store.get_real_path(&store_path), &tx2).await;
            if let Err(err) = err {
                log::error!("Error dumping path {}: {:?}", store_path.display(), err);
            }
        });
        // we keep this closure extra to avoid unaligned copies in the non-range request case.
        task::spawn(async move {
            while let Some(Ok(data)) = rx2.recv().await {
                let len = data.len() as u64;
                if send + len > offset {
                    let start = if send < offset { offset - send } else { 0 };
                    let end = if send + data.len() as u64 > offset + rlength {
                        start + rlength
                    } else {
                        len
                    };
                    // does it fit into usize
                    let start: usize = match start.try_into() {
                        Ok(v) => v,
                        Err(e) => {
                            log::error!("BUG: start(u64) is too big for usize: {:?}", e);
                            break;
                        }
                    };
                    let end: usize = match end.try_into() {
                        Ok(v) => v,
                        Err(e) => {
                            log::error!("BUG: end(u64) is too big for usize: {:?}", e);
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
                log::error!("Error dumping path {}: {:?}", store_path.display(), err);
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
    use std::process::Command;

    async fn dump_to_vec(path: String) -> Result<Vec<u8>> {
        let store = Store::new();
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Result<Bytes, ThreadSafeError>>(1000);
        task::spawn(async move {
            let e = dump_path(store.get_real_path(&PathBuf::from(&path)), &tx).await;
            if let Err(e) = e {
                eprintln!("Error dumping path: {:?}", e);
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
                    bail!("Got error: {:?}", e);
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
            println!("{}", line);
            i += 16;
        }
    }

    #[tokio::test]
    async fn test_dump_store() -> Result<()> {
        let temp_dir = tempfile::tempdir()
            .context("Failed to create temp dir")
            .expect("Failed to create temp dir");
        let dir = temp_dir.path();
        fs::write(dir.join("file"), b"somecontent")?;

        fs::create_dir(dir.join("some_empty_dir"))?;

        let some_dir = dir.join("some_dir");
        fs::create_dir(&some_dir)?;

        let executable_path = some_dir.join("executable");
        fs::write(&executable_path, b"somescript")?;
        fs::set_permissions(&executable_path, fs::Permissions::from_mode(0o755))?;

        std::os::unix::fs::symlink("sometarget", dir.join("symlink"))?;

        let nar_dump = dump_to_vec(dir.to_str().unwrap().to_owned()).await?;
        let res = Command::new("nix-store")
            .arg("--dump")
            .arg(dir)
            .output()
            .context("Failed to run nix-store --dump")?;
        assert_eq!(res.status.code(), Some(0));
        println!("nar_dump:");
        pretty_hex_dump(&nar_dump);
        println!("nix-store --dump:");
        pretty_hex_dump(&res.stdout);
        assert_eq!(res.stdout, nar_dump);

        Ok(())
    }
}
