//! Filesystem-backed [`FileSystemSource`] and [`FileSystemSink`] via
//! [`cap-tokio`].
//!
//! All navigation uses `openat`/`fstatat` syscalls — no path assembly
//! and no symlink following on intermediate components.
//!
//! # Read side ([`DirSource`])
//!
//! [`DirSource::entries`](FileSystemSource::entries) collects and sorts
//! entry names + metadata but does NOT open child directory handles.
//! Each child is returned as a lazy `Entry` thunk — the actual `openat`
//! only happens when you call [`entries`](FileSystemSource::entries) or
//! [`open`](FileSystemSource::open) on the child.
//!
//! [`DirSource::read_file`](FileSystemSource::read_file) uses
//! memory-mapped IO for files larger than 256 KiB (see [`mmap`]).
//!
//! # Write side ([`DirSlotSink`])
//!
//! [`DirSlotSink`] creates a node at a slot (parent dir + name).
//! [`DirDirSink`] populates a directory's children one at a time.
//! [`DirFileSink`] streams file contents via [`AsyncWrite`](tokio::io::AsyncWrite).

pub mod mmap;
mod sink;
mod source;

pub use sink::{DirDirSink, DirFileSink, DirSlotSink};
pub use source::{DirSource, DirSourceEntries, DirSourceError, FileReader};

#[cfg(unix)]
fn is_executable(meta: &cap_tokio::fs::Metadata) -> bool {
    use cap_tokio::fs::MetadataExt;
    meta.mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_meta: &cap_tokio::fs::Metadata) -> bool {
    false
}
