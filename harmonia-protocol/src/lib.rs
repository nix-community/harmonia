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

pub mod aterm;
pub mod build_result;
pub mod daemon_wire;
pub mod de;
pub mod log;
pub mod nar_hash;
pub mod ser;
pub mod types;
pub mod valid_path_info;
pub mod version;

pub use nar_hash::NarHash;

pub use version::ProtocolVersion;

// Re-exports required by derive macros (harmonia_protocol_derive generates code using crate::store_path, etc.)
pub use harmonia_store_core::store_path;

// Hand-written serialization impls for harmonia-store-core types
mod store_impls;

// Re-export structure for code that references harmonia_protocol::daemon::...
pub mod daemon {
    pub use crate::daemon_wire::logger::{FutureResultExt, ResultLog, ResultLogExt};
    pub use crate::daemon_wire::{self as wire, logger};
    pub use crate::daemon_wire::{IgnoredTrue, IgnoredZero};
    pub use crate::de;
    pub use crate::ser;
    pub use crate::types::*;
    pub use crate::version::*;
}
