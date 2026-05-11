//! Generic file tree types and serde.
//!
//! The core types are [`FileSystemObject`] (tagged enum of regular file,
//! directory, or symlink) and [`FileTree`] (recursive newtype wrapper).
//!
//! Serde produces JSON matching `nix nar ls --json`:
//! ```json
//! { "type": "directory", "entries": { "foo": { "type": "regular", "size": 15 } } }
//! ```

mod serde_impl;

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

/// An opaque placeholder used in shallow listings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Opaque;

/// A shallow (one-level) file tree — directory children are [`Opaque`].
pub type ShallowTree<C> = FileSystemObject<C, Opaque>;
