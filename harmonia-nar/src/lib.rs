// SPDX-FileCopyrightText: 2024 griff
// SPDX-FileCopyrightText: 2025 Jörg Thalheim
// SPDX-License-Identifier: EUPL-1.2 OR MIT
//
// This crate is derived from Nix.rs (https://github.com/griff/Nix.rs)
// Upstream commit: f5d129b71bb30b476ce21e6da2a53dcb28607a89

//! NAR (Nix ARchive) format handling.
//!
//! This crate provides functionality for packing and unpacking NAR archives,
//! the archive format used by Nix for representing store paths as byte streams.
//!
//! **Architecture**: This is the Format Layer in Harmonia's store architecture.
//! See `docs/architecture/harmonia-store-structure.md` for details.
//!
//! # Key Features
//!
//! - Streaming NAR pack/unpack (bounded memory usage)
//! - Async/await support via tokio
//! - Works with any `AsyncRead`/`AsyncWrite` source/sink
//! - NAR hash computation during streaming
//!
//! # Design Principles
//!
//! 1. **Streaming**: Never require entire NAR in memory
//! 2. **IO-agnostic**: Work with trait objects (AsyncRead/AsyncWrite)
//! 3. **Format-focused**: Only concerned with archive structure
//! 4. **Composable**: Can be used independently of daemon

/// Byte string type alias.
pub type ByteString = bytes::Bytes;

/// Wire protocol utilities for NAR format.
pub use harmonia_utils_io::wire;

pub mod padded_reader;

pub mod archive;

// Re-export commonly used types from archive
pub use archive::{
    CASE_HACK_SUFFIX, DumpOptions, DumpedFile, NarByteStream, NarDumper, NarEvent, NarParser,
    NarReader, NarRestorer, NarWriteError, NarWriter, RestoreOptions, dump, parse_nar, restore,
};

#[cfg(test)]
pub mod test;
