// SPDX-FileCopyrightText: 2024 griff
// SPDX-FileCopyrightText: 2025 Jörg Thalheim
// SPDX-License-Identifier: EUPL-1.2 OR MIT
//
// This crate is derived from Nix.rs (https://github.com/griff/Nix.rs)
// Upstream commit: f5d129b71bb30b476ce21e6da2a53dcb28607a89

//! Nix daemon wire protocol.
//!
//! This crate defines the types and serialization format for the Nix daemon protocol,
//! enabling communication between clients and the daemon server.
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

// TODO T012b: Re-export types from store-core for use in protocol code
pub use harmonia_store_core::{
    ByteString, derivation, derived_path, hash, io, log, realisation, signature, store_path,
};

// Re-export archive from nar (optional dependency - TODO T012b)
#[cfg(feature = "harmonia-nar")]
pub use harmonia_nar as archive;

pub mod de;
pub mod ser;
pub mod types;
pub mod version;
pub mod wire;

// Re-export commonly used types to crate root for internal use
pub use version::ProtocolVersion;

// Daemon wire protocol (from daemon/wire)
pub mod daemon_wire;

// Hand-written serialization impls for harmonia-store-core types (T012e)
// These have custom logic that can't be expressed with derives
mod store_impls;

// Daemon module structure (for code that references crate::daemon::...)
// TODO T012b: Review this entire re-export structure and simplify
pub mod daemon {
    pub use crate::daemon_wire::{self as wire, logger};
    pub use crate::de;
    pub use crate::ser;
    pub use crate::types::*;
    pub use crate::version::*;

    // Re-export commonly used wire types and logger traits
    // TODO T012b: Review if these should be re-exported at daemon:: level
    pub use crate::daemon_wire::logger::{FutureResultExt, ResultLog, ResultLogExt};
    pub use crate::daemon_wire::{IgnoredTrue, IgnoredZero};

    // TODO T012b: These submodule re-exports are temporary workarounds
    // Re-export submodules for code that uses crate::daemon::types::*, crate::daemon::version::*
    pub mod types {
        pub use crate::types::*;
    }
    pub mod version {
        pub use crate::version::*;
    }
}

// TODO T012b: Re-export derive macros and remote macros
pub use harmonia_protocol_derive::{
    NixDeserialize, NixSerialize, nix_deserialize_remote, nix_serialize_remote,
};
