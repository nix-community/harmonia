//! ValidPathInfo types for the daemon protocol.
//!
//! The pure types and JSON serialization live in `harmonia-store-path-info`.
//! This module re-exports them and derives `NixDeserialize`/`NixSerialize`
//! for the daemon wire protocol.

use std::collections::BTreeSet;

use harmonia_store_core::content_address::ContentAddress;
use harmonia_store_path::{StoreDir, StorePath};
use harmonia_utils_signature::Signature;

// Re-export the pure types so existing users of
// `harmonia_protocol::valid_path_info::*` continue to work.
pub use harmonia_store_path_info::{NarHash, Pure, UnkeyedValidPathInfo, ValidPathInfo};

// Derive NixDeserialize/NixSerialize for the external types.
// The struct bodies mirror the definitions in harmonia-store-path-info;
// only the trait impls are emitted (not the struct definitions).
harmonia_protocol_derive::nix_derive_for! {
    #[nix(for_type = "harmonia_store_path_info::UnkeyedValidPathInfo")]
    struct UnkeyedValidPathInfo {
        pub deriver: Option<StorePath>,
        pub nar_hash: NarHash,
        pub references: BTreeSet<StorePath>,
        pub registration_time: Option<core::num::NonZero<i64>>,
        pub nar_size: u64,
        pub ultimate: bool,
        pub signatures: BTreeSet<Signature>,
        pub ca: Option<ContentAddress>,
        pub store_dir: StoreDir,
    }
}

harmonia_protocol_derive::nix_derive_for! {
    #[nix(for_type = "harmonia_store_path_info::ValidPathInfo")]
    struct ValidPathInfo {
        pub path: StorePath,
        pub info: UnkeyedValidPathInfo,
    }
}
