// SPDX-FileCopyrightText: 2024 griff
// SPDX-FileCopyrightText: 2025 Jörg Thalheim
// SPDX-License-Identifier: EUPL-1.2 OR MIT
//
// This crate is derived from Nix.rs (https://github.com/griff/Nix.rs)
// Upstream commit: f5d129b71bb30b476ce21e6da2a53dcb28607a89

//! NAR (Nix ARchive) format handling through [`harmonia-file-core`] traits.
//!
//! # Dump and restore
//!
//! [`dump_source`] writes a NAR archive from any [`FileSystemSource`] to
//! an [`AsyncWrite`](tokio::io::AsyncWrite). [`restore_to_sink`] parses
//! a NAR archive from any [`AsyncBytesRead`](harmonia_utils_io::AsyncBytesRead)
//! and writes to any [`FileSystemSink`].
//!
//! ```rust,ignore
//! // Dump a DirSource to NAR bytes
//! dump_source(&dir_source, &mut writer).await?;
//!
//! // Restore NAR bytes into a MemoryTree
//! restore_to_sink(reader, builder.sink()).await?;
//! ```
//!
//! # Listing
//!
//! [`parse_nar_listing`] produces a [`FileTree<NarFileInfo>`] from a
//! NAR stream — the same JSON format as `nix nar ls --json --recursive`.
//!
//! # Streaming
//!
//! [`NarByteStream`] produces a `Stream<Item = Bytes>` of NAR-encoded
//! data for a filesystem path, suitable for HTTP streaming.
//!
//! # Design principles
//!
//! 1. **Streaming**: Never require entire NAR in memory
//! 2. **Trait-based**: Dump/restore go through [`FileSystemSource`]/[`FileSystemSink`]
//! 3. **Format-focused**: Only concerned with archive structure
//! 4. **Composable**: Can be used independently of daemon
//!
//! [`FileSystemSource`]: harmonia_file_io_pure::FileSystemSource
//! [`FileSystemSink`]: harmonia_file_io_pure::FileSystemSink

/// Byte string type alias.
pub type ByteString = bytes::Bytes;

/// Wire protocol utilities for NAR format.
pub use harmonia_utils_io::wire;

pub mod listing;
pub mod padded_reader;

pub mod archive;

// Re-export commonly used types from archive
pub use archive::{
    CASE_HACK_SUFFIX, NarByteStream, NarEvent, NarParser, NarReader, NarWriter, dump_source,
    parse_nar, restore_to_sink,
};
pub use listing::{NarFileInfo, parse_nar_listing};

#[cfg(test)]
pub mod test;
