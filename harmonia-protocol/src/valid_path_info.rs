//! ValidPathInfo types for the daemon protocol.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
#[cfg(test)]
use test_strategy::Arbitrary;

use crate::NarHash;
use harmonia_protocol_derive::{NixDeserialize, NixSerialize};
use harmonia_store_core::signature::Signature;
#[cfg(test)]
use harmonia_store_core::signature::proptests::arb_signatures;
use harmonia_store_core::store_path::{ContentAddress, StoreDir, StorePath};

use crate::types::DaemonTime;

#[derive(Debug, Clone, PartialEq, Eq, Hash, NixDeserialize, NixSerialize)]
#[cfg_attr(test, derive(Arbitrary))]
pub struct UnkeyedValidPathInfo {
    pub deriver: Option<StorePath>,
    pub nar_hash: NarHash,
    pub references: BTreeSet<StorePath>,
    pub registration_time: DaemonTime,
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

// JSON serialization format version 2
// This matches upstream Nix's JSON format for path-info

fn is_false(b: &bool) -> bool {
    !b
}

fn is_empty<T>(set: &BTreeSet<T>) -> bool {
    set.is_empty()
}

fn is_zero(t: &DaemonTime) -> bool {
    *t == 0
}

/// Helper struct for JSON serialization/deserialization of UnkeyedValidPathInfo
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawUnkeyedValidPathInfo {
    // ca is always included (even when null) in upstream format
    ca: Option<ContentAddress>,
    #[serde(skip_serializing_if = "Option::is_none")]
    deriver: Option<StorePath>,
    nar_hash: NarHash,
    nar_size: u64,
    #[serde(default)]
    references: BTreeSet<StorePath>,
    #[serde(skip_serializing_if = "is_zero", default)]
    registration_time: DaemonTime,
    #[serde(skip_serializing_if = "is_empty", default)]
    signatures: BTreeSet<Signature>,
    store_dir: StoreDir,
    #[serde(skip_serializing_if = "is_false", default)]
    ultimate: bool,
    version: u32,
}

impl Serialize for UnkeyedValidPathInfo {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let raw = RawUnkeyedValidPathInfo {
            ca: self.ca,
            deriver: self.deriver.clone(),
            nar_hash: self.nar_hash,
            nar_size: self.nar_size,
            references: self.references.clone(),
            registration_time: self.registration_time,
            signatures: self.signatures.clone(),
            store_dir: self.store_dir.clone(),
            ultimate: self.ultimate,
            version: 2,
        };
        raw.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for UnkeyedValidPathInfo {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = RawUnkeyedValidPathInfo::deserialize(deserializer)?;

        // Validate version
        if raw.version != 2 {
            return Err(serde::de::Error::custom(format!(
                "unsupported path-info version: {}, expected 2",
                raw.version
            )));
        }

        Ok(UnkeyedValidPathInfo {
            deriver: raw.deriver,
            nar_hash: raw.nar_hash,
            references: raw.references,
            registration_time: raw.registration_time,
            nar_size: raw.nar_size,
            ultimate: raw.ultimate,
            signatures: raw.signatures,
            ca: raw.ca,
            store_dir: raw.store_dir,
        })
    }
}
