use crate::error::{CacheError, Result, StoreError};
use actix_web::{HttpResponse, http, web};
use harmonia_store_core::store_path::{StoreDir, StorePathHash};
use harmonia_store_db::ValidPathInfo;
use harmonia_utils_hash::Hash;
use serde::Deserialize;

use crate::config::Config;
use crate::{cache_control_max_age_1d, some_or_404};
use harmonia_store_core::signature::{SecretKey, fingerprint_path};

#[derive(Debug, Deserialize)]
pub struct Param {
    json: Option<String>,
}

/// NarInfo wraps a `ValidPathInfo` with narinfo-specific fields.
#[derive(Debug)]
struct NarInfo {
    /// The underlying path info (with all signatures including cache sigs).
    info: ValidPathInfo,
    /// URL to fetch the NAR.
    url: String,
    /// Compression method (e.g. "none").
    compression: String,
    /// Hash of the (possibly compressed) file. Same as nar_hash when uncompressed.
    file_hash: Option<Hash>,
    /// Size of the (possibly compressed) file. Same as nar_size when uncompressed.
    file_size: Option<u64>,
}

/// Build a `NarInfo` from a `ValidPathInfo`, signing with the cache keys.
fn build_narinfo(
    store_dir: &StoreDir,
    mut info: ValidPathInfo,
    hash: &str,
    sign_keys: &[SecretKey],
) -> Result<NarInfo> {
    use harmonia_utils_hash::fmt::CommonHash as _;

    let nar_hash_obj: Hash = info.info.nar_hash.into();
    let nar_hash = format!("{}", nar_hash_obj.as_base32()).into_bytes();
    let nar_hash_bare = format!("{}", nar_hash_obj.as_base32().as_bare());

    let url = format!("nar/{nar_hash_bare}.nar?hash={hash}");

    // Sign with the cache's secret keys and add to the signatures set.
    let fingerprint = fingerprint_path(
        store_dir,
        &info.path,
        &nar_hash,
        info.info.nar_size,
        &info.info.references,
    )?;
    for sk in sign_keys {
        info.info.signatures.insert(sk.sign(&fingerprint));
    }

    Ok(NarInfo {
        info,
        url,
        compression: "none".into(),
        file_hash: None,
        file_size: None,
    })
}

/// Helper macro for adding lines to narinfo
macro_rules! push_line {
    ($buf:expr, $prefix:literal, $value:expr) => {
        $buf.extend_from_slice($prefix);
        $buf.extend_from_slice($value);
        $buf.push(b'\n');
    };
}

fn format_narinfo_txt(store_dir: &StoreDir, narinfo: &NarInfo) -> Vec<u8> {
    use harmonia_utils_hash::fmt::CommonHash as _;

    let path = &narinfo.info.path;
    let pi = &narinfo.info.info;

    let nar_hash_obj: Hash = pi.nar_hash.into();
    let nar_hash_str = format!("{}", nar_hash_obj.as_base32());
    let nar_size_str = pi.nar_size.to_string();

    let store_path_display = store_dir.display(path).to_string();

    let mut result = Vec::new();

    let file_hash_str = narinfo
        .file_hash
        .map(|h| format!("{}", h.as_base32()))
        .unwrap_or_else(|| nar_hash_str.clone());
    let file_size_str = narinfo
        .file_size
        .map(|s| s.to_string())
        .unwrap_or_else(|| nar_size_str.clone());

    // Required fields
    push_line!(result, b"StorePath: ", store_path_display.as_bytes());
    push_line!(result, b"URL: ", narinfo.url.as_bytes());
    push_line!(result, b"Compression: ", narinfo.compression.as_bytes());
    push_line!(result, b"FileHash: ", file_hash_str.as_bytes());
    push_line!(result, b"FileSize: ", file_size_str.as_bytes());
    push_line!(result, b"NarHash: ", nar_hash_str.as_bytes());
    push_line!(result, b"NarSize: ", nar_size_str.as_bytes());

    // References
    if !pi.references.is_empty() {
        result.extend_from_slice(b"References:");
        for r in &pi.references {
            result.push(b' ');
            result.extend_from_slice(r.to_string().as_bytes());
        }
        result.push(b'\n');
    }

    // Optional fields
    if let Some(drv) = &pi.deriver {
        push_line!(result, b"Deriver: ", drv.to_string().as_bytes());
    }

    // All signatures (DB sigs + cache signing key sigs).
    for sig in &pi.signatures {
        push_line!(result, b"Sig: ", sig.to_string().as_bytes());
    }

    if let Some(ca) = &pi.ca {
        push_line!(result, b"CA: ", ca.to_string().as_bytes());
    }

    result
}

pub(crate) async fn get(
    hash: web::Path<String>,
    param: web::Query<Param>,
    settings: web::Data<Config>,
) -> crate::ServerResult {
    let hash = hash.into_inner();
    // Reject malformed hash parts up front so the `path >= ?` index scan in
    // SQLite cannot be tricked into matching an unrelated store path.
    let store_path_hash = StorePathHash::decode_digest(hash.as_bytes()).map_err(|e| {
        CacheError::from(StoreError::PathQuery {
            hash: hash.clone(),
            reason: format!("Invalid hash format: {e}"),
        })
    })?;

    let info = some_or_404!(
        settings
            .store
            .query_path_info_by_hash_part(&store_path_hash)?
    );
    let narinfo = build_narinfo(
        settings.store.store_dir(),
        info,
        &hash,
        &settings.secret_keys,
    )?;

    if param.json.is_some() {
        // JSON format: return the underlying path info.
        Ok(HttpResponse::Ok()
            .insert_header(cache_control_max_age_1d())
            .json(&narinfo.info.info))
    } else {
        let url = narinfo.url.clone();
        let res = format_narinfo_txt(settings.store.store_dir(), &narinfo);
        Ok(HttpResponse::Ok()
            .insert_header((http::header::CONTENT_TYPE, "text/x-nix-narinfo"))
            .insert_header(("Nix-Link", url))
            .insert_header(cache_control_max_age_1d())
            .body(res))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use harmonia_store_path_info::{NarHash, UnkeyedValidPathInfo};
    use std::collections::BTreeSet;

    #[test]
    fn test_format_narinfo_minimal() {
        let path: harmonia_store_core::store_path::StorePath =
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa0-test".parse().unwrap();
        let narinfo = NarInfo {
            info: ValidPathInfo {
                id: 1,
                path: path.clone(),
                info: UnkeyedValidPathInfo {
                    deriver: None,
                    nar_hash: NarHash::from_slice(&[0u8; 32]).unwrap(),
                    references: BTreeSet::new(),
                    registration_time: None,
                    nar_size: 1234,
                    ultimate: false,
                    signatures: BTreeSet::new(),
                    ca: None,
                    store_dir: StoreDir::default(),
                },
            },
            url: "nar/abc123.nar?hash=test".into(),
            compression: "none".into(),
            file_hash: None,
            file_size: None,
        };

        let result = format_narinfo_txt(&StoreDir::default(), &narinfo);
        let result_str = String::from_utf8_lossy(&result);

        let lines: Vec<&str> = result_str.trim().split('\n').collect();
        assert!(lines[0].starts_with("StorePath: /nix/store/"));
        assert_eq!(lines[1], "URL: nar/abc123.nar?hash=test");
        assert_eq!(lines[2], "Compression: none");
        assert!(lines[3].starts_with("FileHash: sha256:"));
        assert_eq!(lines[4], "FileSize: 1234");
        assert!(lines[5].starts_with("NarHash: sha256:"));
        assert_eq!(lines[6], "NarSize: 1234");
        assert_eq!(lines.len(), 7);
    }
}
