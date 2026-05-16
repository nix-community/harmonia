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
pub use store_dir::{FromStoreDirStr, StoreDir, StoreDirDisplay};

pub type StorePathSet = BTreeSet<StorePath>;

#[cfg(any(test, feature = "test"))]
pub mod proptest {
    pub use super::path::proptest::*;
    pub use super::store_dir::proptest::*;
}
