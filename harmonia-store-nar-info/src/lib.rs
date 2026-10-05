//! NarInfo construction and formatting for the Nix binary cache protocol.

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::num::NonZero;

use serde::{Deserialize, Serialize, Serializer};

use harmonia_store_content_address::ContentAddress;
use harmonia_store_path::{StoreDir, StorePath};
use harmonia_store_path_info::{
    NarHash, StorePathKeyed, UnkeyedValidPathInfo, ValidPathInfo, fingerprint_path,
};
use harmonia_utils_hash::{Hash, fmt::Any};
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
    use harmonia_utils_hash::HashFormat as _;

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
    use harmonia_utils_hash::HashFormat as _;

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

/// Errors from parsing the textual narinfo format.
#[derive(Debug, thiserror::Error)]
pub enum NarInfoParseError {
    #[error("line {line}: expected 'Key: value'")]
    MalformedLine { line: usize },
    #[error("line {line}: invalid {field} ({message})")]
    InvalidField {
        line: usize,
        field: &'static str,
        message: String,
    },
    #[error("duplicate {field} field")]
    Duplicate { field: &'static str },
    #[error("missing required {field} field")]
    Missing { field: &'static str },
}

/// Parses the textual narinfo format, reading `StorePath` as a full path and `References`/`Deriver` as base names.
pub fn parse_narinfo_txt(store_dir: &StoreDir, s: &str) -> Result<NarInfo, NarInfoParseError> {
    let mut path = Option::<StorePath>::None;
    let mut url = Option::<String>::None;
    let mut compression = Option::<String>::None;
    let mut download_hash = Option::<Hash>::None;
    let mut download_size = Option::<u64>::None;
    let mut nar_hash = Option::<NarHash>::None;
    let mut nar_size = Option::<u64>::None;
    let mut references = BTreeSet::<StorePath>::new();
    let mut have_references = false;
    let mut deriver = Option::<StorePath>::None;
    let mut signatures = BTreeSet::<Signature>::new();
    let mut ca = Option::<ContentAddress>::None;

    for (i, line) in s.lines().enumerate() {
        if line.is_empty() {
            continue;
        }
        let line_no = i + 1;
        let (key, value) = line
            .split_once(": ")
            .ok_or(NarInfoParseError::MalformedLine { line: line_no })?;
        let invalid =
            |field: &'static str, e: &dyn std::fmt::Display| NarInfoParseError::InvalidField {
                line: line_no,
                field,
                message: e.to_string(),
            };
        match key {
            "StorePath" => {
                if path.is_some() {
                    return Err(NarInfoParseError::Duplicate { field: "StorePath" });
                }
                path = Some(
                    store_dir
                        .parse::<StorePath>(value)
                        .map_err(|e| invalid("StorePath", &e))?,
                );
            }
            "URL" => url = Some(value.to_owned()),
            "Compression" => compression = Some(value.to_owned()),
            // Nix accepts any encoding here, and Cachix writes base16.
            "FileHash" => {
                download_hash = Some(
                    value
                        .parse::<Any<Hash>>()
                        .map_err(|e| invalid("FileHash", &e))?
                        .into_hash(),
                );
            }
            "FileSize" => {
                download_size = Some(value.parse::<u64>().map_err(|e| invalid("FileSize", &e))?);
            }
            "NarHash" => {
                // Parse as `Hash`, which has no implied algorithm, so that the
                // `sha256:` prefix is required like in Nix.
                let hash = value
                    .parse::<Any<Hash>>()
                    .map_err(|e| invalid("NarHash", &e))?
                    .into_hash();
                nar_hash = Some(NarHash::try_from(hash).map_err(|e| invalid("NarHash", &e))?);
            }
            "NarSize" => {
                nar_size = Some(value.parse::<u64>().map_err(|e| invalid("NarSize", &e))?);
            }
            "References" => {
                if have_references {
                    return Err(NarInfoParseError::Duplicate {
                        field: "References",
                    });
                }
                have_references = true;
                for r in value.split_whitespace() {
                    references.insert(
                        StorePath::from_bytes(r.as_bytes())
                            .map_err(|e| invalid("References", &e))?,
                    );
                }
            }
            "Deriver" if value != "unknown-deriver" => {
                deriver = Some(
                    StorePath::from_bytes(value.as_bytes()).map_err(|e| invalid("Deriver", &e))?,
                );
            }
            "Sig" => {
                signatures.insert(value.parse::<Signature>().map_err(|e| invalid("Sig", &e))?);
            }
            "CA" => {
                if ca.is_some() {
                    return Err(NarInfoParseError::Duplicate { field: "CA" });
                }
                ca = Some(
                    value
                        .parse::<ContentAddress>()
                        .map_err(|e| invalid("CA", &e))?,
                );
            }
            // Unknown keys are ignored
            _ => {}
        }
    }

    Ok(NarInfo {
        path: path.ok_or(NarInfoParseError::Missing { field: "StorePath" })?,
        info: UnkeyedNarInfo {
            info: UnkeyedValidPathInfo {
                deriver,
                nar_hash: nar_hash.ok_or(NarInfoParseError::Missing { field: "NarHash" })?,
                references,
                registration_time: None,
                nar_size: nar_size.ok_or(NarInfoParseError::Missing { field: "NarSize" })?,
                ultimate: false,
                signatures,
                ca,
                store_dir: store_dir.clone(),
            },
            url: Some(url.ok_or(NarInfoParseError::Missing { field: "URL" })?),
            compression,
            download_hash,
            download_size,
        },
    })
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
    use harmonia_utils_hash::HashView as _;
    use rstest::rstest;

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

    fn sample_nar_hash() -> NarHash {
        use harmonia_utils_hash::fmt::Base32;
        "sha256:1b4sb93wp679q4zx9k1ignby1yna3z7c4c2ri3wphylbc2dwsys0"
            .parse::<Base32<NarHash>>()
            .unwrap()
            .into_hash()
    }

    #[test]
    fn test_narinfo_text_round_trip() {
        let store_dir = StoreDir::default();
        let path: StorePath = "55xkmqns51sw7nrgykp5vnz36w4fr3cw-nix-2.1.3"
            .parse()
            .unwrap();
        let dep: StorePath = "0jqd0rlxzra1rs38rdxl43yh6rxchgc6-curl-7.82.0"
            .parse()
            .unwrap();
        let nar_hash = sample_nar_hash();
        let mut references = BTreeSet::new();
        references.insert(dep.clone());

        let original = NarInfo {
            path,
            info: UnkeyedNarInfo {
                info: UnkeyedValidPathInfo {
                    deriver: Some(dep),
                    nar_hash,
                    references,
                    registration_time: None,
                    nar_size: 196_040,
                    ultimate: false,
                    signatures: BTreeSet::new(),
                    ca: None,
                    store_dir: store_dir.clone(),
                },
                url: Some("nar/abc.nar.xz".into()),
                compression: Some("xz".into()),
                download_hash: Some(Hash::from(nar_hash)),
                download_size: Some(12_345),
            },
        };

        let text = format_narinfo_txt(&store_dir, &original);
        let parsed = parse_narinfo_txt(&store_dir, std::str::from_utf8(&text).unwrap()).unwrap();
        assert_eq!(parsed, original);
    }

    const SHA256: [u8; 32] =
        hex_literal::hex!("407bcd9b608b7a78f9885930c2ce1fcafae0977d31ccd43fc1e998cb475a9aac");
    const SHA1: [u8; 20] = hex_literal::hex!("da39a3ee5e6b4b0d3255bfef95601890afd80709");
    const B32: &str = "1b4sb93wp679q4zx9k1ignby1yna3z7c4c2ri3wphylbc2dwsys0";
    const HEX: &str = "407bcd9b608b7a78f9885930c2ce1fcafae0977d31ccd43fc1e998cb475a9aac";
    const B64: &str = "QHvNm2CLenj5iFkwws4fyvrgl30xzNQ/wemYy0damqw=";
    const SHA1_HEX: &str = "da39a3ee5e6b4b0d3255bfef95601890afd80709";

    /// Parses a valid narinfo in which `field` is replaced by `value`, or dropped if `None`.
    fn parse_with(field: &str, value: Option<&str>) -> Result<NarInfo, NarInfoParseError> {
        let defaults = [
            (
                "StorePath",
                "/nix/store/55xkmqns51sw7nrgykp5vnz36w4fr3cw-nix-2.1.3",
            ),
            ("URL", "nar/abc.nar.zst"),
            ("Compression", "zstd"),
            ("FileHash", &format!("sha256:{B32}")),
            ("FileSize", "12345"),
            ("NarHash", &format!("sha256:{B32}")),
            ("NarSize", "196040"),
        ];
        let mut fields: Vec<(&str, &str)> = defaults
            .into_iter()
            .filter(|(name, _)| *name != field)
            .collect();
        fields.extend(value.map(|v| (field, v)));
        let text: String = fields.iter().map(|(n, v)| format!("{n}: {v}\n")).collect();
        parse_narinfo_txt(&StoreDir::default(), &text)
    }

    // Nix accepts any encoding (Cachix writes base16) but requires the type prefix.
    // Only FileHash may be something other than sha256.
    #[rstest]
    #[case::base32(format!("sha256:{B32}"), &SHA256, true)]
    #[case::base16(format!("sha256:{HEX}"), &SHA256, true)]
    #[case::base16_upper(format!("sha256:{}", HEX.to_uppercase()), &SHA256, true)]
    #[case::base64(format!("sha256:{B64}"), &SHA256, true)]
    #[case::sri(format!("sha256-{B64}"), &SHA256, true)]
    #[case::sha1(format!("sha1:{SHA1_HEX}"), &SHA1, false)]
    fn test_parse_hash_accepted(
        #[values("FileHash", "NarHash")] field: &str,
        #[case] value: String,
        #[case] digest: &[u8],
        #[case] allowed_in_nar_hash: bool,
    ) {
        let result = parse_with(field, Some(&value));
        if field == "NarHash" && !allowed_in_nar_hash {
            assert!(matches!(
                result,
                Err(NarInfoParseError::InvalidField {
                    field: "NarHash",
                    ..
                })
            ));
            return;
        }
        let info = result.unwrap();
        let actual = match field {
            "FileHash" => info.info.download_hash.unwrap(),
            _ => info.info.info.nar_hash.into(),
        };
        assert_eq!(actual.digest_bytes(), digest);
        assert_eq!(
            info.path.to_string(),
            "55xkmqns51sw7nrgykp5vnz36w4fr3cw-nix-2.1.3"
        );
    }

    #[rstest]
    #[case::bare_base32(B32.to_owned())]
    #[case::bare_base16(HEX.to_owned())]
    #[case::bare_base64(B64.to_owned())]
    #[case::unpadded_base64(format!("sha256:{}", B64.trim_end_matches('=')))]
    #[case::wrong_length("sha256:abcd".to_owned())]
    fn test_parse_hash_rejected(
        #[values("FileHash", "NarHash")] field: &'static str,
        #[case] value: String,
    ) {
        assert!(matches!(
            parse_with(field, Some(&value)),
            Err(NarInfoParseError::InvalidField { field: f, .. }) if f == field
        ));
    }

    #[rstest]
    #[case("StorePath")]
    #[case("URL")]
    #[case("NarHash")]
    #[case("NarSize")]
    fn test_parse_missing_field(#[case] field: &'static str) {
        assert!(matches!(
            parse_with(field, None),
            Err(NarInfoParseError::Missing { field: f }) if f == field
        ));
    }
}
