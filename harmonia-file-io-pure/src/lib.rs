//! Async file tree IO traits, in-memory implementations, and listing functions.
//!
//! This does not do any "real" IO, but unlike `harmonia-file-core`,
//! it does use `async` and creates some slightly opinionated IO
//! abstractions.
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
//! See also [`harmonia-file-core`](harmonia_file_core) for the
//! underlying data types.

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
