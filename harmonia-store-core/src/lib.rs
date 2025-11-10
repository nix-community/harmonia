// SPDX-FileCopyrightText: 2024 griff
// SPDX-FileCopyrightText: 2025 Jörg Thalheim
// SPDX-License-Identifier: EUPL-1.2 OR MIT
//
// This crate is derived from Nix.rs (https://github.com/griff/Nix.rs)
// Upstream commit: f5d129b71bb30b476ce21e6da2a53dcb28607a89

//! Core Nix store semantics.
//!
//! This crate provides the fundamental types and pure computation logic for working
//! with the Nix store. It is intentionally IO-free - all operations are pure functions
//! that operate on values, enabling easy testing and composition.
//!
//! **Architecture**: This is the Core Layer in Harmonia's store architecture.
//! See `docs/architecture/harmonia-store-structure.md` for details.
//!
//! # Key Modules
//!
//! - `hash` - Content addressing, hash types, hash computation
//! - `store_path` - Store path types, parsing, validation
//! - `derivation` - Derivation (.drv) file format and semantics
//! - `signature` - Cryptographic signatures for store paths
//! - `realisation` - Store path realisation tracking
//!
//! # Design Principles
//!
//! 1. **No IO**: No filesystem, no network, minimal `async`
//! 2. **Pure functions**: Deterministic, testable, referentially transparent
//! 3. **Explicit errors**: All fallible operations return `Result`
//! 4. **Memory-bounded**: Stream-friendly, no unbounded buffers

// Type alias for byte strings
pub type ByteString = bytes::Bytes;

// Serde helpers for ByteString
pub(crate) fn serialize_byte_string<S>(value: &ByteString, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    use serde::Serialize;
    let bytes: &[u8] = value.as_ref();
    bytes.serialize(serializer)
}

pub(crate) fn deserialize_byte_string<'de, D>(deserializer: D) -> Result<ByteString, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;
    let bytes: Vec<u8> = Vec::deserialize(deserializer)?;
    Ok(ByteString::from(bytes))
}

// Wire utilities (duplicated from harmonia-protocol to avoid circular deps)
pub mod wire {
    pub const ZEROS: [u8; 8] = [0u8; 8];

    pub const fn base64_len(len: usize) -> usize {
        ((4 * len / 3) + 3) & !3
    }

    pub const fn calc_aligned(len: u64) -> u64 {
        len.wrapping_add(7) & !7
    }

    pub const fn calc_padding(len: u64) -> usize {
        let aligned = calc_aligned(len);
        aligned.wrapping_sub(len) as usize
    }
}

pub mod base32;
pub mod derivation;
pub mod derived_path;
pub mod hash;
pub mod io;
pub mod log;
pub mod realisation;
pub mod signature;
pub mod store_path;

#[cfg(any(test, feature = "test"))]
pub mod test;
