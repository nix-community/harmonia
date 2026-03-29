//! ValidPathInfo types for the daemon protocol.

use std::{borrow::Cow, collections::BTreeSet, num::NonZero};

use serde::{Deserialize, Serialize, Serializer};
#[cfg(test)]
use test_strategy::Arbitrary;

use crate::NarHash;
use harmonia_protocol_derive::{NixDeserialize, NixSerialize};
use harmonia_store_core::signature::Signature;
#[cfg(test)]
use harmonia_store_core::signature::proptests::arb_signatures;
use harmonia_store_core::store_path::{ContentAddress, StoreDir, StorePath};

use crate::types::DaemonTime;

/// Serializes in "pure" format, omitting impure fields when they have default values.
/// Used for content-addressed paths.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Pure<T>(pub T);

#[derive(Debug, Clone, PartialEq, Eq, Hash, NixDeserialize, NixSerialize)]
#[cfg_attr(test, derive(Arbitrary))]
pub struct UnkeyedValidPathInfo {
    pub deriver: Option<StorePath>,
    pub nar_hash: NarHash,
    pub references: BTreeSet<StorePath>,
    pub registration_time: Option<NonZero<DaemonTime>>,
    pub nar_size: u64,
    pub ultimate: bool,
    #[cfg_attr(test, strategy(arb_signatures()))]
    pub signatures: BTreeSet<Signature>,
    pub ca: Option<ContentAddress>,
    pub store_dir: StoreDir,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, NixDeserialize, NixSerialize)]
#[cfg_attr(test, derive(Arbitrary))]
pub struct ValidPathInfo {
    pub path: StorePath,
    pub info: UnkeyedValidPathInfo,
}

// JSON format version 2, matching upstream Nix

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
    registration_time: Option<DaemonTime>,
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
    registration_time: Option<DaemonTime>,
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
        version: 2,
    };
    raw.serialize(serializer)
}

fn deserialize_info<'de, D>(deserializer: D) -> Result<UnkeyedValidPathInfo, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = RawUnkeyedValidPathInfo::deserialize(deserializer)?;
    if raw.version != 2 {
        return Err(serde::de::Error::custom(format!(
            "unsupported path-info version: {}, expected 2",
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
            version: 2,
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
