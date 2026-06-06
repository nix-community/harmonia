// SPDX-FileCopyrightText: 2024 griff
// SPDX-FileCopyrightText: 2026 Jörg Thalheim
// SPDX-License-Identifier: EUPL-1.2 OR MIT
//
// This crate is derived from Nix.rs (https://github.com/griff/Nix.rs)
// Upstream commit: f5d129b71bb30b476ce21e6da2a53dcb28607a89

//! Nix daemon wire protocol.
//!
//! This crate defines the types and serialization format for the Nix daemon protocol,
//! enabling communication between clients and the daemon server. It also defines the
//! [`DaemonStore`](`daemon::DaemonStore`) operation interface and higher-level store
//! operations built on it.
//!
//! **Architecture**: This is the Protocol Layer in Harmonia's store architecture.
//! See `docs/architecture/harmonia-store-structure.md` for details.
//!
//! # Key Features
//!
//! - Protocol message types (handshake, operations, responses)
//! - Versioned protocol support
//! - Efficient serialization/deserialization
//! - Derive macros for protocol types
//!
//! # Design Principles
//!
//! 1. **Versioned**: Support protocol version negotiation
//! 2. **Backward-compatible**: Handle older protocol versions
//! 3. **Well-specified**: Document wire format
//! 4. **Type-safe**: Use strong types for protocol messages

mod build_result;
pub mod daemon_wire;
pub mod de;
pub mod log;
pub mod ser;
pub mod types;
pub mod valid_path_info;
pub mod version;

pub use harmonia_store_path_info::NarHash;

pub use version::{
    FEATURE_REALISATION_WITH_PATH, Feature, FeatureSet, ProtocolVersion, supported_features,
};

// Re-exports required by derive macros (harmonia_protocol_derive generates code using crate::store_path, etc.)
// The module exposes both the content-address types from harmonia-store-content-address and the
// store-path types (FromStoreDirStr, StoreDirDisplay, etc.) from harmonia-store-path.
pub mod store_path {
    pub use harmonia_store_content_address::{
        ContentAddress, ContentAddressMethod, ContentAddressMethodAlgorithm,
        ParseContentAddressError, make_store_path_from_ca,
    };
    pub use harmonia_store_path::{
        FromStoreDirStr, ParseStorePathError, StoreDir, StoreDirDisplay, StorePath, StorePathError,
        StorePathHash, StorePathName, StorePathNameError, StorePathSet, into_name,
    };
}

// Hand-written serialization impls for harmonia-store-derivation types
mod store_impls;

/// Higher-level store operations built on the [`DaemonStore`](`daemon::DaemonStore`) trait.
pub mod store_ops;

#[cfg(test)]
mod wire_roundtrip;

// Re-export structure for code that references harmonia_protocol::daemon::...
pub mod daemon {
    pub use crate::daemon_wire::logger::{FutureResultExt, ResultLog, ResultLogExt};
    pub use crate::daemon_wire::{self as wire, logger};
    pub use crate::daemon_wire::{IgnoredTrue, IgnoredZero};
    pub use crate::de;
    pub use crate::ser;
    pub use crate::store_ops::write_derivation;
    pub use crate::types::*;
    pub use crate::version::*;
}
