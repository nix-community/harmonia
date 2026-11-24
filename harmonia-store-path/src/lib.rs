// SPDX-FileCopyrightText: 2024 griff
// SPDX-FileCopyrightText: 2025 Jörg Thalheim
// SPDX-License-Identifier: EUPL-1.2 OR MIT
//
// This crate is derived from Nix.rs (https://github.com/griff/Nix.rs)
// Upstream commit: f5d129b71bb30b476ce21e6da2a53dcb28607a89

//! Nix store path types, parsing, and validation.
//!
//! Part of the Store (pure) layer — see `docs/architecture/harmonia-store-structure.md`.

use std::collections::BTreeSet;

mod path;
mod store_dir;

pub use path::into_name;
pub use path::{
    ParseStorePathError, StorePath, StorePathError, StorePathHash, StorePathName,
    StorePathNameError,
};
pub use store_dir::{FromStoreDirStr, StoreDir, StoreDirDisplay, StoreDirError};

pub type StorePathSet = BTreeSet<StorePath>;

pub(crate) fn deserialize_from_str<'de, D, T>(
    deserializer: D,
    expecting: &'static str,
) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    struct Visitor<T> {
        expecting: &'static str,
        _marker: std::marker::PhantomData<T>,
    }

    impl<T> serde::de::Visitor<'_> for Visitor<T>
    where
        T: std::str::FromStr,
        T::Err: std::fmt::Display,
    {
        type Value = T;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str(self.expecting)
        }

        fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            v.parse().map_err(serde::de::Error::custom)
        }
    }

    deserializer.deserialize_str(Visitor {
        expecting,
        _marker: std::marker::PhantomData,
    })
}

#[cfg(any(test, feature = "test"))]
pub mod proptest {
    pub use super::path::proptest::*;
    pub use super::store_dir::proptest::*;
}
