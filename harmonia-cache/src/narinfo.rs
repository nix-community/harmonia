use crate::error::{CacheError, NarInfoError, Result, StoreError};
use actix_web::{HttpResponse, http, web};
use harmonia_store_core::store_path::{StoreDir, StorePath, StorePathHash};
use harmonia_store_db::ValidPathInfo;
use harmonia_utils_hash::Hash;
use harmonia_utils_hash::fmt::{Any, CommonHash};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

use crate::config::Config;
use crate::{cache_control_max_age_1d, some_or_404};
use harmonia_store_core::signature::{SecretKey, fingerprint_path};

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

/// Build a `NarInfo` from a SQLite `ValidPaths` row.
fn build_narinfo(
    store_dir: &StoreDir,
    virtual_nix_store: &[u8],
    info: &ValidPathInfo,
    hash: &str,
    sign_keys: &[SecretKey],
) -> Result<NarInfo> {
    let store_path: StorePath =
        store_dir
            .parse(&info.path)
            .map_err(|e| NarInfoError::QueryFailed {
                reason: format!("invalid store path in db: {e}"),
            })?;

    // The db stores the nar hash as `sha256:<base16>`; narinfo and the
    // signature fingerprint want `sha256:<base32>`.
    let nar_hash_parsed: Hash = info
        .hash
        .parse::<Any<Hash>>()
        .map_err(|e| NarInfoError::QueryFailed {
            reason: format!("invalid nar hash '{}' in db: {e}", info.hash),
        })?
        .into_hash();
    let nar_hash = format!("{}", nar_hash_parsed.as_base32()).into_bytes();
    let nar_hash_bare = format!("{}", nar_hash_parsed.as_base32().as_bare()).into_bytes();
    let nar_size = info.nar_size.ok_or_else(|| NarInfoError::QueryFailed {
        reason: format!("missing narSize for {}", info.path),
    })?;

    let store_path_str = store_path.to_string();
    let full_store_path = crate::build_bytes!(virtual_nix_store, b"/", store_path_str.as_bytes(),);

    let mut references: BTreeSet<StorePath> = BTreeSet::new();
    for r in &info.references {
        let sp: StorePath = store_dir.parse(r).map_err(|e| NarInfoError::QueryFailed {
            reason: format!("invalid reference '{r}' in db: {e}"),
        })?;
        references.insert(sp);
    }

    let deriver = info.deriver.as_deref().and_then(|d| {
        store_dir
            .parse::<StorePath>(d)
            .ok()
            .map(|sp| sp.to_string().into_bytes())
    });

    let mut res = NarInfo {
        store_path: full_store_path,
        url: crate::build_bytes!(b"nar/", &nar_hash_bare, b".nar?hash=", hash.as_bytes(),),
        compression: b"none".to_vec(),
        nar_hash: nar_hash.clone(),
        nar_size,
        references: references
            .iter()
            .map(|r| r.to_string().into_bytes())
            .collect(),
        deriver,
        sigs: vec![],
        ca: info.ca.as_ref().map(|ca| ca.clone().into_bytes()),
    };

    let fingerprint =
        fingerprint_path(store_dir, &store_path, &res.nar_hash, nar_size, &references)?;
    for sk in sign_keys {
        res.sigs
            .push(sk.sign(&fingerprint).to_string().into_bytes());
    }
    if res.sigs.is_empty() {
        res.sigs = info
            .signatures()
            .iter()
            .map(|s| s.as_bytes().to_vec())
            .collect();
    }

    Ok(res)
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
    // Reject malformed hash parts up front so the `path >= ?` index scan in
    // SQLite cannot be tricked into matching an unrelated store path.
    let hash = StorePathHash::decode_digest(hash.as_bytes())
        .map_err(|e| {
            CacheError::from(StoreError::PathQuery {
                hash: hash.clone(),
                reason: format!("Invalid hash format: {e}"),
            })
        })?
        .to_string();

    let info = some_or_404!(settings.store.query_path_info_by_hash_part(&hash)?);
    let narinfo = build_narinfo(
        settings.store.store_dir(),
        settings.store.virtual_store(),
        &info,
        &hash,
        &settings.secret_keys,
    )?;

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
