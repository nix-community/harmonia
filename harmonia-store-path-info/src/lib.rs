// SPDX-FileCopyrightText: 2025 Obsidian Systems
// SPDX-License-Identifier: MIT

//! Pure `ValidPathInfo`, `NarHash`, and related types for Nix store metadata.
//!
//! Quarantined from `harmonia-store-core` because `ValidPathInfo` is a
//! bag of loosely related fields that doesn't benefit from being in core.
//! Protocol-specific wire format derives are added in the protocol layer.

mod nar_hash;

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::num::NonZero;

#[cfg(any(test, feature = "test"))]
use proptest_derive::Arbitrary;
use serde::{Deserialize, Serialize, Serializer};

pub use nar_hash::NarHash;

use harmonia_store_core::store_path::{ContentAddress, StoreDir, StorePath};
use harmonia_utils_hash::fmt::CommonHash as _;
use harmonia_utils_signature::Signature;
#[cfg(any(test, feature = "test"))]
use harmonia_utils_signature::proptests::arb_signatures;

/// Generate a fingerprint for signing a store path.
///
/// The fingerprint format is:
/// `1;<store-path>;<nar-hash>;<nar-size>;<comma-separated-references>`
///
/// # Arguments
/// * `store_dir` - The Nix store directory
/// * `store_path` - The store path to fingerprint
/// * `nar_hash` - The NAR hash (always SHA256)
/// * `nar_size` - The size of the NAR in bytes
/// * `references` - Sorted references to other store paths
pub fn fingerprint_path(
    store_dir: &StoreDir,
    store_path: &StorePath,
    nar_hash: &NarHash,
    nar_size: u64,
    references: &BTreeSet<StorePath>,
) -> Vec<u8> {
    let nar_hash_str = format!("{}", nar_hash.as_base32());
    let nar_hash_bytes = nar_hash_str.as_bytes();
    let nar_size_str = nar_size.to_string();
    let nar_size_bytes = nar_size_str.as_bytes();

    // Construct full store path string using StoreDir's display functionality
    let store_path_str = format!("{}", store_dir.display(store_path));
    let store_path_bytes = store_path_str.as_bytes();

    // Calculate capacity
    let fixed_len = 2 + // "1;"
        store_path_bytes.len() + 1 + // store path + ";"
        nar_hash_bytes.len() + 1 + // nar hash + ";"
        nar_size_bytes.len() + 1; // nar size + ";"

    let refs_len = if references.is_empty() {
        0
    } else {
        // Each reference formatted with store_dir
        references
            .iter()
            .map(|r| format!("{}", store_dir.display(r)).len())
            .sum::<usize>()
            + references.len().saturating_sub(1) // commas between refs
    };

    let mut result = Vec::with_capacity(fixed_len + refs_len);

    // Add fixed parts
    result.extend_from_slice(b"1;");
    result.extend_from_slice(store_path_bytes);
    result.push(b';');
    result.extend_from_slice(nar_hash_bytes);
    result.push(b';');
    result.extend_from_slice(nar_size_bytes);
    result.push(b';');

    // Add references (comma-separated)
    for (i, reference) in references.iter().enumerate() {
        if i > 0 {
            result.push(b',');
        }
        let ref_str = format!("{}", store_dir.display(reference));
        result.extend_from_slice(ref_str.as_bytes());
    }

    result
}

/// Serializes in "pure" format, omitting impure fields when they have default values.
/// Used for content-addressed paths.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Pure<T>(pub T);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(any(test, feature = "test"), derive(Arbitrary))]
pub struct UnkeyedValidPathInfo {
    pub deriver: Option<StorePath>,
    pub nar_hash: NarHash,
    pub references: BTreeSet<StorePath>,
    pub registration_time: Option<NonZero<i64>>,
    pub nar_size: u64,
    pub ultimate: bool,
    #[cfg_attr(any(test, feature = "test"), proptest(strategy = "arb_signatures()"))]
    pub signatures: BTreeSet<Signature>,
    pub ca: Option<ContentAddress>,
    pub store_dir: StoreDir,
}

impl UnkeyedValidPathInfo {
    /// Clear impure-only fields, keeping only the content-addressed metadata.
    pub fn into_pure(mut self) -> Self {
        self.deriver = None;
        self.registration_time = None;
        self.ultimate = false;
        self.signatures = BTreeSet::new();
        self
    }
}

/// Pairs a `StorePath` key with some unkeyed payload.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(any(test, feature = "test"), derive(Arbitrary))]
pub struct StorePathKeyed<T> {
    pub path: StorePath,
    pub info: T,
}

/// A valid store path with its metadata.
pub type ValidPathInfo = StorePathKeyed<UnkeyedValidPathInfo>;

// -- JSON serialization (version 3) ------------------------------------------

fn is_false(b: &bool) -> bool {
    !b
}

fn is_none_store_path(opt: &Option<Cow<'_, StorePath>>) -> bool {
    opt.is_none()
}

#[expect(clippy::ptr_arg, reason = "needed for serde skip_serializing_if")]
fn is_empty_signatures(set: &Cow<'_, BTreeSet<Signature>>) -> bool {
    set.is_empty()
}

/// Pure format: omits default impure fields during serialization
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RawUnkeyedValidPathInfoPure<'a> {
    ca: Option<ContentAddress>,
    #[serde(skip_serializing_if = "is_none_store_path")]
    deriver: Option<Cow<'a, StorePath>>,
    nar_hash: NarHash,
    nar_size: u64,
    references: Cow<'a, BTreeSet<StorePath>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    registration_time: Option<i64>,
    #[serde(skip_serializing_if = "is_empty_signatures")]
    signatures: Cow<'a, BTreeSet<Signature>>,
    store_dir: Cow<'a, StoreDir>,
    #[serde(skip_serializing_if = "is_false")]
    ultimate: bool,
    version: u32,
}

fn default_cow_set<T: Ord + Clone>() -> Cow<'static, BTreeSet<T>> {
    Cow::Owned(BTreeSet::new())
}

fn default_cow_store_dir() -> Cow<'static, StoreDir> {
    Cow::Owned(StoreDir::default())
}

/// Impure format: includes all fields, used for both serialize and deserialize
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawUnkeyedValidPathInfo<'a> {
    ca: Option<ContentAddress>,
    #[serde(default)]
    deriver: Option<Cow<'a, StorePath>>,
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
    version: u32,
}

fn serialize_impure<S>(info: &UnkeyedValidPathInfo, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let raw = RawUnkeyedValidPathInfo {
        ca: info.ca,
        deriver: info.deriver.as_ref().map(Cow::Borrowed),
        nar_hash: info.nar_hash,
        nar_size: info.nar_size,
        references: Cow::Borrowed(&info.references),
        registration_time: info.registration_time.map(|n| n.get()),
        signatures: Cow::Borrowed(&info.signatures),
        store_dir: Cow::Borrowed(&info.store_dir),
        ultimate: info.ultimate,
        version: 3,
    };
    raw.serialize(serializer)
}

fn deserialize_info<'de, D>(deserializer: D) -> Result<UnkeyedValidPathInfo, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = RawUnkeyedValidPathInfo::deserialize(deserializer)?;
    if raw.version != 3 {
        return Err(serde::de::Error::custom(format!(
            "unsupported path-info version: {}, expected 3",
            raw.version
        )));
    }
    Ok(UnkeyedValidPathInfo {
        deriver: raw.deriver.map(Cow::into_owned),
        nar_hash: raw.nar_hash,
        references: raw.references.into_owned(),
        registration_time: raw.registration_time.and_then(NonZero::new),
        nar_size: raw.nar_size,
        ultimate: raw.ultimate,
        signatures: raw.signatures.into_owned(),
        ca: raw.ca,
        store_dir: raw.store_dir.into_owned(),
    })
}

/// Default: impure format
impl Serialize for UnkeyedValidPathInfo {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_impure(self, serializer)
    }
}

impl<'de> Deserialize<'de> for UnkeyedValidPathInfo {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserialize_info(deserializer)
    }
}

impl Serialize for Pure<UnkeyedValidPathInfo> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let raw = RawUnkeyedValidPathInfoPure {
            ca: self.0.ca,
            deriver: self.0.deriver.as_ref().map(Cow::Borrowed),
            nar_hash: self.0.nar_hash,
            nar_size: self.0.nar_size,
            references: Cow::Borrowed(&self.0.references),
            registration_time: self.0.registration_time.map(|n| n.get()),
            signatures: Cow::Borrowed(&self.0.signatures),
            store_dir: Cow::Borrowed(&self.0.store_dir),
            ultimate: self.0.ultimate,
            version: 3,
        };
        raw.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Pure<UnkeyedValidPathInfo> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserialize_info(deserializer).map(Pure)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use harmonia_utils_hash::fmt::Base32;

    #[test]
    fn test_fingerprint_path() {
        let store_dir = StoreDir::new("/nix/store").unwrap();
        let store_path =
            StorePath::from_bytes(b"syd87l2rxw8cbsxmxl853h0r6pdwhwjr-curl-7.82.0-bin").unwrap();
        let nar_hash: NarHash = "sha256:1b4sb93wp679q4zx9k1ignby1yna3z7c4c2ri3wphylbc2dwsys0"
            .parse::<Base32<NarHash>>()
            .unwrap()
            .into_hash();
        let mut references = BTreeSet::new();
        references.insert(
            StorePath::from_bytes(b"0jqd0rlxzra1rs38rdxl43yh6rxchgc6-curl-7.82.0").unwrap(),
        );
        let fingerprint = fingerprint_path(&store_dir, &store_path, &nar_hash, 196040, &references);
        let expected = b"1;/nix/store/syd87l2rxw8cbsxmxl853h0r6pdwhwjr-curl-7.82.0-bin;sha256:1b4sb93wp679q4zx9k1ignby1yna3z7c4c2ri3wphylbc2dwsys0;196040;/nix/store/0jqd0rlxzra1rs38rdxl43yh6rxchgc6-curl-7.82.0";
        assert_eq!(fingerprint, expected);
    }
}
