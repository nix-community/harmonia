// SPDX-FileCopyrightText: 2026 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! Error types for store garbage collection.

use std::path::PathBuf;

use thiserror::Error;

/// Result type for store garbage collection.
pub type Result<T> = std::result::Result<T, Error>;

/// Errors that can occur during garbage collection.
#[derive(Debug, Error)]
pub enum Error {
    /// Store database operation failed
    #[error(transparent)]
    Db(#[from] harmonia_store_db::Error),

    /// The store filesystem layout could not be resolved
    #[error(transparent)]
    StoreFs(#[from] harmonia_store_fs::Error),

    /// The configured store dir matches nothing in the database.
    /// Proceeding would make every path look dead and wipe the store.
    #[error(
        "store dir {store_dir} does not match any path in the Nix database \
         (e.g. {example}), refusing to collect garbage"
    )]
    StoreDirMismatch { store_dir: PathBuf, example: String },

    /// Opening or acquiring the exclusive `gc.lock` failed. This is the
    /// lock Nix and this crate use to serialize garbage collections
    /// against each other and against builders registering roots.
    #[error("GC lock {path}: {source}")]
    GcLock {
        path: PathBuf,
        source: std::io::Error,
    },

    /// Scanning a GC roots directory failed. Skipping it could hide
    /// live roots and delete paths that are still in use.
    #[error("scanning roots in {dir}: {source}")]
    ScanRoots {
        dir: PathBuf,
        #[source]
        source: Box<Error>,
    },

    /// Reading a directory failed
    #[error("reading directory {path}: {source}")]
    ReadDir {
        path: PathBuf,
        source: std::io::Error,
    },

    /// stat() on a path failed
    #[error("stat {path}: {source}")]
    Stat {
        path: PathBuf,
        source: std::io::Error,
    },

    /// readlink() on a path failed
    #[error("readlink {path}: {source}")]
    ReadLink {
        path: PathBuf,
        source: std::io::Error,
    },

    /// Opening or reading a temp roots file failed
    #[error("temp roots file {path}: {source}")]
    TempRootFile {
        path: PathBuf,
        source: std::io::Error,
    },

    /// Writing to this process's own temp roots file failed
    #[error("writing temp root: {source}")]
    TempRootWrite { source: std::io::Error },

    /// Connecting or talking to a running GC's socket failed
    #[error("gc-socket {path}: {source}")]
    GcSocketClient {
        path: PathBuf,
        source: std::io::Error,
    },

    /// Creating the gc-socket directory or socket failed
    #[error("serving gc-socket at {path}: {source}")]
    GcSocketServe {
        path: PathBuf,
        source: std::io::Error,
    },

    /// Spawning a GC worker thread failed
    #[error("spawning {name} thread: {source}")]
    SpawnThread {
        name: &'static str,
        source: std::io::Error,
    },

    /// A gc-socket client sent an over-long line. Store paths are short,
    /// and the socket is world-writable like Nix's, so an unbounded line
    /// could only be an attempt to exhaust the collector's memory.
    #[error("gc-socket: line too long")]
    LineTooLong,

    /// Reading from or writing to a gc-socket client failed
    #[error("gc-socket client: {source}")]
    ClientIo { source: std::io::Error },

    /// Opening or acquiring a profile lock failed
    #[error("profile lock {path}: {source}")]
    ProfileLock {
        path: PathBuf,
        source: std::io::Error,
    },

    /// A time spec like "30d" could not be parsed
    #[error("invalid time spec '{spec}': {reason}")]
    TimeSpec { spec: String, reason: String },

    /// Deleting a store path failed
    #[error("removing {path}: {source}")]
    Remove {
        path: PathBuf,
        source: std::io::Error,
    },
}
