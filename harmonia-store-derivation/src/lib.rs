// SPDX-FileCopyrightText: 2024 griff
// SPDX-FileCopyrightText: 2026 Jörg Thalheim
// SPDX-License-Identifier: EUPL-1.2 OR MIT
//
// This crate is derived from Nix.rs (https://github.com/griff/Nix.rs)
// Upstream commit: f5d129b71bb30b476ce21e6da2a53dcb28607a89

//! Nix derivation types and semantics.
//!
//! This crate provides derivations, derived paths, placeholders, and
//! realisations — the pure computation logic for Nix build planning.
//!
//! # Key Modules
//!
//! - `derivation` - Derivation (.drv) file format and semantics
//! - `derived_path` - References to derivation outputs
//! - `placeholder` - Placeholder strings for self-referencing derivations
//! - `realisation` - Store path realisation tracking
//!
//! Part of the Store (pure) layer — see `docs/architecture/harmonia-store-structure.md`.

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

#[cfg(any(test, feature = "test"))]
pub mod test;
