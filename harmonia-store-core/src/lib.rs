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

/// Implement `serde::Serialize`/`Deserialize` via existing `Display`/`FromStr` impls.
///
/// Replaces the `SerializeDisplay`/`DeserializeFromStr` derives from `serde_with`
/// without pulling in that crate's heavy proc-macro dependency stack (darling).
#[macro_export]
macro_rules! impl_serde_via_string {
    ($t:ty) => {
        impl ::serde::Serialize for $t {
            fn serialize<S: ::serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.collect_str(self)
            }
        }
        impl<'de> ::serde::Deserialize<'de> for $t {
            fn deserialize<D: ::serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let s = <String as ::serde::Deserialize>::deserialize(d)?;
                s.parse().map_err(::serde::de::Error::custom)
            }
        }
    };
}

pub mod derivation;
pub mod derived_path;
pub mod placeholder;
pub mod realisation;
pub mod signature;
pub mod store_path;

#[cfg(any(test, feature = "test"))]
pub mod test;
