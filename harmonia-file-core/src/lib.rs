//! Generic file tree types, async traits, and listing functions.
//!
//! # Read side
//!
//! [`FileSystemSource`] represents a node in a file tree. You navigate
//! to children with [`open`](FileSystemSource::open) and iterate them
//! with [`entries`](FileSystemSource::entries). The entries stream
//! yields `(name, ChildThunk)` pairs where the thunk is a future that
//! produces the opened child — so directory handles are only opened on
//! demand.
//!
//! # Write side
//!
//! [`FileSystemSink`] creates a single node (file, dir, or symlink).
//! For directories, it returns a [`DirectorySink`] whose
//! [`create_child`](DirectorySink::create_child) method yields a
//! sub-sink for each child — one level at a time, matching the NAR
//! format's depth-first structure. [`RegularFileSink`] implements
//! [`AsyncWrite`](tokio::io::AsyncWrite) for streaming file contents.
//!
//! # In-memory implementation
//!
//! [`MemoryTreeSource`] and [`MemoryTreeBuilder`] provide a pure
//! in-memory implementation useful for testing and for building
//! trees programmatically (e.g. from parsed NARs).
//!
//! # Serde
//!
//! [`FileSystemObject`] and [`FileTree`] derive
//! `Serialize`/`Deserialize` with `#[serde(tag = "type")]`, producing
//! JSON that matches `nix nar ls --json`.
//!
//! [NixOS/nix#15392]: https://github.com/NixOS/nix/pull/15392

mod canon_path;
mod listing;
mod serde_impl;
mod sink;
mod source;

#[cfg(test)]
mod tests;

pub use canon_path::{CanonPath, CanonPathError};
pub use listing::{Opaque, ShallowTree, list_deep, list_shallow};
pub use sink::{DirectorySink, FileSystemSink, MemoryTreeBuilder, RegularFileSink};
pub use source::{FileSystemSource, FileType, MemoryTreeSource, Stat};

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// File-system object types
// ---------------------------------------------------------------------------

/// A regular file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Regular<C> {
    #[serde(default)]
    pub executable: bool,
    #[serde(flatten)]
    pub contents: C,
}

/// A directory whose children are of type `Child`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Directory<Child> {
    pub entries: BTreeMap<String, Child>,
}

/// A symbolic link.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Symlink {
    pub target: String,
}

/// A file-system object, generic over content type `C` and child type `Ch`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum FileSystemObject<C, Ch> {
    #[serde(rename = "regular")]
    Regular(Regular<C>),
    #[serde(rename = "directory")]
    Directory(Directory<Ch>),
    #[serde(rename = "symlink")]
    Symlink(Symlink),
}

/// A fully recursive file tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileTree<C>(pub FileSystemObject<C, Box<FileTree<C>>>);

/// An in-memory file tree with byte-vector contents.
pub type MemoryTree = FileTree<Vec<u8>>;
