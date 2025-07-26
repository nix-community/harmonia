use std::path::Path;

use crate::error::{CacheError, NarInfoError, Result, StoreError};
use actix_web::{HttpResponse, http, web};
use harmonia_store_remote::protocol::StorePath;
use serde::{Deserialize, Serialize};
use std::os::unix::ffi::OsStrExt;

use crate::config::{Config, SigningKey};
use crate::signing::{fingerprint_path, sign_string};
use crate::{cache_control_max_age_1d, nixhash, some_or_404};

#[derive(Debug, Deserialize)]
pub struct Param {
    json: Option<String>,
}

#[derive(Debug, Serialize)]
struct NarInfo {
    store_path: Vec<u8>,
    url: Vec<u8>,
    compression: Vec<u8>,
    nar_hash: Vec<u8>,
    nar_size: u64,
    references: Vec<Vec<u8>>,
    deriver: Option<Vec<u8>>,
    sigs: Vec<Vec<u8>>,
    ca: Option<Vec<u8>>,
}

fn extract_filename(path: &[u8]) -> Option<Vec<u8>> {
    Path::new(std::ffi::OsStr::from_bytes(path))
        .file_name()
        .map(|v| v.as_bytes().to_vec())
}

async fn query_narinfo(
    virtual_nix_store: &[u8],
    store_path: &StorePath,
    hash: &str,
    sign_keys: &Vec<SigningKey>,
    settings: &web::Data<Config>,
) -> Result<Option<NarInfo>> {
    let mut daemon_guard = settings.store.get_daemon().await?;
    let daemon = daemon_guard.as_mut().unwrap();

    let path_info = match daemon
        .query_path_info(store_path)
        .await
        .map_err(|e| CacheError::from(StoreError::Remote(e)))?
    {
        Some(info) => info,
        None => {
            return Ok(None);
        }
    };
    let nar_hash = path_info.hash.to_nix_base32();
    let mut res = NarInfo {
        store_path: store_path.as_bytes().to_vec(),
        url: crate::build_bytes!(b"nar/", &nar_hash, b".nar?hash=", hash.as_bytes(),),
        compression: b"none".to_vec(),
        nar_hash: crate::build_bytes!(b"sha256:", &nar_hash,),
        nar_size: path_info.nar_size,
        references: vec![],
        deriver: path_info
            .deriver
            .as_ref()
            .and_then(|d| extract_filename(d.as_bytes())),
        sigs: vec![],
        ca: path_info.content_address.clone(),
    };

    if !path_info.references.is_empty() {
        res.references = path_info
            .references
            .iter()
            .filter_map(|r| extract_filename(r.as_bytes()))
            .collect::<Vec<Vec<u8>>>();
    }

    let fingerprint = fingerprint_path(
        virtual_nix_store,
        store_path,
        &res.nar_hash,
        res.nar_size,
        &path_info.references,
    )?;
    for sk in sign_keys {
        if let Some(ref fp) = fingerprint {
            res.sigs.push(sign_string(sk, fp));
        }
    }

    if res.sigs.is_empty() {
        res.sigs = path_info.signatures.clone();
    }

    Ok(Some(res))
}

/// Helper macro for adding lines to narinfo
macro_rules! push_line {
    ($buf:expr, $prefix:literal, $value:expr) => {
        $buf.extend_from_slice($prefix);
        $buf.extend_from_slice($value);
        $buf.push(b'\n');
    };
}

fn format_narinfo_txt(narinfo: &NarInfo) -> Vec<u8> {
    let nar_size_str = narinfo.nar_size.to_string();
    let nar_size_bytes = nar_size_str.as_bytes();

    // Pre-calculate capacity
    let mut capacity = 0;
    capacity += 11 + narinfo.store_path.len() + 1;
    capacity += 5 + narinfo.url.len() + 1;
    capacity += 13 + narinfo.compression.len() + 1;
    capacity += 10 + narinfo.nar_hash.len() + 1;
    capacity += 10 + nar_size_bytes.len() + 1;
    capacity += 9 + narinfo.nar_hash.len() + 1;
    capacity += 9 + nar_size_bytes.len() + 1;

    if !narinfo.references.is_empty() {
        capacity += 12
            + narinfo
                .references
                .iter()
                .map(|r| r.len() + 1)
                .sum::<usize>();
    }

    if let Some(drv) = &narinfo.deriver {
        capacity += 9 + drv.len() + 1;
    }

    capacity += narinfo
        .sigs
        .iter()
        .map(|sig| 5 + sig.len() + 1)
        .sum::<usize>();

    if let Some(ca) = &narinfo.ca {
        capacity += 4 + ca.len() + 1;
    }

    let mut result = Vec::with_capacity(capacity);

    // Required fields
    push_line!(result, b"StorePath: ", &narinfo.store_path);
    push_line!(result, b"URL: ", &narinfo.url);
    push_line!(result, b"Compression: ", &narinfo.compression);
    push_line!(result, b"FileHash: ", &narinfo.nar_hash);
    push_line!(result, b"FileSize: ", nar_size_bytes);
    push_line!(result, b"NarHash: ", &narinfo.nar_hash);
    push_line!(result, b"NarSize: ", nar_size_bytes);

    // References
    if !narinfo.references.is_empty() {
        result.extend_from_slice(b"References:");
        for r in &narinfo.references {
            result.push(b' ');
            result.extend_from_slice(r);
        }
        result.push(b'\n');
    }

    // Optional fields
    if let Some(drv) = &narinfo.deriver {
        push_line!(result, b"Deriver: ", drv);
    }

    for sig in &narinfo.sigs {
        push_line!(result, b"Sig: ", sig);
    }

    if let Some(ca) = &narinfo.ca {
        push_line!(result, b"CA: ", ca);
    }

    result
}

pub(crate) async fn get(
    hash: web::Path<String>,
    param: web::Query<Param>,
    settings: web::Data<Config>,
) -> crate::ServerResult {
    let hash = hash.into_inner();
    let real_store_path =
        some_or_404!(
            nixhash(&settings, hash.as_bytes())
                .await
                .map_err(|e| CacheError::from(NarInfoError::QueryFailed {
                    reason: format!("Could not query nar hash in database: {e}"),
                }))?
        );

    // Convert real store path to virtual store path
    let store_path = settings.store.to_virtual_path(&real_store_path);

    let narinfo = match query_narinfo(
        settings.store.virtual_store(),
        &store_path,
        &hash,
        &settings.secret_keys,
        &settings,
    )
    .await?
    {
        Some(narinfo) => narinfo,
        None => {
            return Ok(HttpResponse::NotFound()
                .insert_header(cache_control_max_age_1d())
                .body("missed hash"));
        }
    };

    if param.json.is_some() {
        Ok(HttpResponse::Ok()
            .insert_header(cache_control_max_age_1d())
            .json(narinfo))
    } else {
        let res = format_narinfo_txt(&narinfo);
        Ok(HttpResponse::Ok()
            .insert_header((http::header::CONTENT_TYPE, "text/x-nix-narinfo"))
            .insert_header(("Nix-Link", narinfo.url))
            .insert_header(cache_control_max_age_1d())
            .body(res))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_narinfo_minimal() {
        let narinfo = NarInfo {
            store_path: b"/nix/store/abc123-test".to_vec(),
            url: b"nar/abc123.nar?hash=test".to_vec(),
            compression: b"none".to_vec(),
            nar_hash: b"sha256:0000000000000000000000000000000000000000000000000000".to_vec(),
            nar_size: 1234,
            references: vec![],
            deriver: None,
            sigs: vec![],
            ca: None,
        };

        let result = format_narinfo_txt(&narinfo);
        let result_str = String::from_utf8_lossy(&result);

        let lines: Vec<&str> = result_str.trim().split('\n').collect();
        assert_eq!(lines[0], "StorePath: /nix/store/abc123-test");
        assert_eq!(lines[1], "URL: nar/abc123.nar?hash=test");
        assert_eq!(lines[2], "Compression: none");
        assert_eq!(
            lines[3],
            "FileHash: sha256:0000000000000000000000000000000000000000000000000000"
        );
        assert_eq!(lines[4], "FileSize: 1234");
        assert_eq!(
            lines[5],
            "NarHash: sha256:0000000000000000000000000000000000000000000000000000"
        );
        assert_eq!(lines[6], "NarSize: 1234");
        assert_eq!(lines.len(), 7);
    }
}
