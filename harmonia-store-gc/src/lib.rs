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
//! ```no_run
//! # fn main() -> harmonia_store_gc::Result<()> {
//! use std::path::Path;
//! use harmonia_store_gc::{GcOptions, GcStore, collect_garbage};
//!
//! let store = GcStore::open(Path::new("/nix/store"), Path::new("/nix/var/nix"))?;
//! let report = collect_garbage(
//!     &store,
//!     &GcOptions {
//!         dry_run: true,
//!         ..Default::default()
//!     },
//! )?;
//! for path in &report.would_delete {
//!     println!("{path}");
//! }
//! println!("~{} bytes in {} paths", report.bytes_freed, report.paths_deleted);
//! # Ok(())
//! # }
//! ```

mod error;
mod gc;
mod gc_socket;
pub mod profiles;
mod roots;
mod store;
mod temp_roots;
#[cfg(test)]
mod testutil;

pub use error::{Error, Result};
pub use gc::{GcOptions, GcReport, collect_garbage};
pub use harmonia_store_db::GraphOptions;
pub use store::GcStore;
pub use temp_roots::TempRoots;

/// Hash map keyed by store path strings.
///
/// SipHash showed up in GC profiles when hashing millions of ~50-char
/// store paths, so foldhash is used instead.
pub(crate) type HashMap<K, V> = std::collections::HashMap<K, V, foldhash::fast::RandomState>;
/// Hash set counterpart of [`HashMap`].
pub(crate) type HashSet<K> = std::collections::HashSet<K, foldhash::fast::RandomState>;
