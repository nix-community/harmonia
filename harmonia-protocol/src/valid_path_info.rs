//! ValidPathInfo types for the daemon protocol.

use std::collections::BTreeSet;

#[cfg(test)]
use test_strategy::Arbitrary;

use harmonia_protocol_derive::{NixDeserialize, NixSerialize};
use harmonia_store_core::signature::Signature;
#[cfg(test)]
use harmonia_store_core::signature::proptests::arb_signatures;
use harmonia_store_core::store_path::{ContentAddress, StorePath};
use harmonia_utils_hash::NarHash;

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
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, NixDeserialize, NixSerialize)]
#[cfg_attr(test, derive(Arbitrary))]
pub struct ValidPathInfo {
    pub path: StorePath,
    pub info: UnkeyedValidPathInfo,
}
