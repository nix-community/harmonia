//! NarInfo construction and formatting for the Nix binary cache protocol.

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::num::NonZero;

use serde::{Deserialize, Serialize, Serializer};

use harmonia_store_core::store_path::{ContentAddress, StoreDir, StorePath};
use harmonia_store_path_info::{
    NarHash, StorePathKeyed, UnkeyedValidPathInfo, ValidPathInfo, fingerprint_path,
};
use harmonia_utils_hash::Hash;
use harmonia_utils_signature::{SecretKey, Signature};

/// A keyed NarInfo: store path plus narinfo metadata.
pub type NarInfo = StorePathKeyed<UnkeyedNarInfo>;

/// Unkeyed NarInfo: path info metadata plus narinfo-specific fields, without
/// a store path key or database ID. Used for JSON serialization matching the
/// upstream Nix format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnkeyedNarInfo {
    /// The underlying path metadata.
    pub info: UnkeyedValidPathInfo,
    /// URL to fetch the NAR.
    pub url: Option<String>,
    /// Compression method (e.g. "xz", "none").
    pub compression: Option<String>,
    /// Hash of the (possibly compressed) download file.
    pub download_hash: Option<Hash>,
    /// Size of the (possibly compressed) download file.
    pub download_size: Option<u64>,
}

/// Build a `NarInfo` from a `ValidPathInfo`, signing with the cache keys.
pub fn build_narinfo(
    store_dir: &StoreDir,
    mut info: ValidPathInfo,
    hash: &str,
    sign_keys: &[SecretKey],
) -> NarInfo {
    use harmonia_utils_hash::fmt::CommonHash as _;

    let nar_hash_obj: Hash = info.info.nar_hash.into();
    let nar_hash_bare = format!("{}", nar_hash_obj.as_base32().as_bare());

    let url = format!("nar/{nar_hash_bare}.nar?hash={hash}");

    // Sign with the cache's secret keys and add to the signatures set.
    let fingerprint = fingerprint_path(
        store_dir,
        &info.path,
        &info.info.nar_hash,
        info.info.nar_size,
        &info.info.references,
    );
    for sk in sign_keys {
        info.info.signatures.insert(sk.sign(&fingerprint));
    }

    NarInfo {
        path: info.path,
        info: UnkeyedNarInfo {
            info: info.info,
            url: Some(url),
            compression: Some("none".into()),
            download_hash: None,
            download_size: None,
        },
    }
}

/// Helper macro for adding lines to narinfo
macro_rules! push_line {
    ($buf:expr, $prefix:literal, $value:expr) => {
        $buf.extend_from_slice($prefix);
        $buf.extend_from_slice($value);
        $buf.push(b'\n');
    };
}

/// Format a `NarInfo` as the textual narinfo format used by the binary cache protocol.
pub fn format_narinfo_txt(store_dir: &StoreDir, narinfo: &NarInfo) -> Vec<u8> {
    use harmonia_utils_hash::fmt::CommonHash as _;

    let path = &narinfo.path;
    let ni = &narinfo.info;
    let pi = &ni.info;

    let nar_hash_obj: Hash = pi.nar_hash.into();
    let nar_hash_str = format!("{}", nar_hash_obj.as_base32());
    let nar_size_str = pi.nar_size.to_string();

    let store_path_display = store_dir.display(path).to_string();

    let mut result = Vec::new();

    let file_hash_str = ni
        .download_hash
        .map(|h| format!("{}", h.as_base32()))
        .unwrap_or_else(|| nar_hash_str.clone());
    let file_size_str = ni
        .download_size
        .map(|s| s.to_string())
        .unwrap_or_else(|| nar_size_str.clone());

    let url = ni.url.as_deref().unwrap_or("");
    let compression = ni.compression.as_deref().unwrap_or("none");

    // Required fields
    push_line!(result, b"StorePath: ", store_path_display.as_bytes());
    push_line!(result, b"URL: ", url.as_bytes());
    push_line!(result, b"Compression: ", compression.as_bytes());
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

// -- JSON serialization (version 3) ------------------------------------------

fn is_none_string(opt: &Option<Cow<'_, str>>) -> bool {
    opt.is_none()
}

fn is_none_hash(opt: &Option<Hash>) -> bool {
    opt.is_none()
}

fn is_none_u64(opt: &Option<u64>) -> bool {
    opt.is_none()
}

fn default_cow_set<T: Ord + Clone>() -> Cow<'static, BTreeSet<T>> {
    Cow::Owned(BTreeSet::new())
}

fn default_cow_store_dir() -> Cow<'static, StoreDir> {
    Cow::Owned(StoreDir::default())
}

/// Impure format: includes all fields.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawUnkeyedNarInfo<'a> {
    ca: Option<ContentAddress>,
    #[serde(default, skip_serializing_if = "is_none_string")]
    compression: Option<Cow<'a, str>>,
    #[serde(default)]
    deriver: Option<Cow<'a, StorePath>>,
    #[serde(default, skip_serializing_if = "is_none_hash")]
    download_hash: Option<Hash>,
    #[serde(default, skip_serializing_if = "is_none_u64")]
    download_size: Option<u64>,
    nar_hash: NarHash,
    nar_size: u64,
    #[serde(default = "default_cow_set")]
    references: Cow<'a, BTreeSet<StorePath>>,
    #[serde(default)]
    registration_time: Option<i64>,
    #[serde(default = "default_cow_set")]
    signatures: Cow<'a, BTreeSet<Signature>>,
    #[serde(default = "default_cow_store_dir")]
    store_dir: Cow<'a, StoreDir>,
    #[serde(default)]
    ultimate: bool,
    #[serde(default, skip_serializing_if = "is_none_string")]
    url: Option<Cow<'a, str>>,
    version: u32,
}

impl Serialize for UnkeyedNarInfo {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let raw = RawUnkeyedNarInfo {
            ca: self.info.ca,
            compression: self.compression.as_deref().map(Cow::Borrowed),
            deriver: self.info.deriver.as_ref().map(Cow::Borrowed),
            download_hash: self.download_hash,
            download_size: self.download_size,
            nar_hash: self.info.nar_hash,
            nar_size: self.info.nar_size,
            references: Cow::Borrowed(&self.info.references),
            registration_time: self.info.registration_time.map(|n| n.get()),
            signatures: Cow::Borrowed(&self.info.signatures),
            store_dir: Cow::Borrowed(&self.info.store_dir),
            ultimate: self.info.ultimate,
            url: self.url.as_deref().map(Cow::Borrowed),
            version: 3,
        };
        raw.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for UnkeyedNarInfo {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = RawUnkeyedNarInfo::deserialize(deserializer)?;
        if raw.version != 3 {
            return Err(serde::de::Error::custom(format!(
                "unsupported nar-info version: {}, expected 3",
                raw.version
            )));
        }
        Ok(UnkeyedNarInfo {
            info: UnkeyedValidPathInfo {
                deriver: raw.deriver.map(Cow::into_owned),
                nar_hash: raw.nar_hash,
                references: raw.references.into_owned(),
                registration_time: raw.registration_time.and_then(NonZero::new),
                nar_size: raw.nar_size,
                ultimate: raw.ultimate,
                signatures: raw.signatures.into_owned(),
                ca: raw.ca,
                store_dir: raw.store_dir.into_owned(),
            },
            url: raw.url.map(|c| c.into_owned()),
            compression: raw.compression.map(|c| c.into_owned()),
            download_hash: raw.download_hash,
            download_size: raw.download_size,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_narinfo_minimal() {
        let path: StorePath = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa0-test".parse().unwrap();
        let narinfo = NarInfo {
            path: path.clone(),
            info: UnkeyedNarInfo {
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
                url: Some("nar/abc123.nar?hash=test".into()),
                compression: Some("none".into()),
                download_hash: None,
                download_size: None,
            },
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
