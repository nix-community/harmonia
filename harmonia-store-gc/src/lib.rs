// SPDX-FileCopyrightText: 2026 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! Garbage collection for the Nix store.
//!
//! A faster drop-in replacement for `nix-collect-garbage`. Instead of one
//! SQLite query per store path, the reference graph is loaded once into
//! memory (see [`harmonia_store_db::StoreGraph`]) and liveness is computed
//! as a graph closure. Deletion runs in parallel.
//!
//! The crate speaks the same on-disk protocols as Nix itself. It takes
//! the same `gc.lock`, honors the temp-root files that running builds
//! register, and serves the `gc-socket` that builders use to protect new
//! paths while a collection is in progress. Running it next to a busy
//! nix-daemon is safe.
//!
//! Entry point: [`gc::collect_garbage`] with a [`store::GcStore`].

mod error;
pub mod roots;
pub mod store;
pub mod temp_roots;

pub use error::{Error, Result};

/// Hash map keyed by store path strings.
///
/// SipHash showed up in GC profiles when hashing millions of ~50-char
/// store paths, so foldhash is used instead.
pub type HashMap<K, V> = std::collections::HashMap<K, V, foldhash::fast::RandomState>;
/// Hash set counterpart of [`HashMap`].
pub(crate) type HashSet<K> = std::collections::HashSet<K, foldhash::fast::RandomState>;
