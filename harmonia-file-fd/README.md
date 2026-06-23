# harmonia-file-fd

Filesystem-backed `FileSystemSource` and `FileSystemSink` via this crate's async `cap-std` wrapper.

## Overview

This crate provides capability-based async filesystem access through
the `harmonia-file-io-pure` traits. All navigation uses `openat`/`fstatat`
syscalls — no path assembly and no symlink following on intermediate
components.

## Read side (`DirSource`)

`DirSource` wraps a `cap_tokio::fs::Dir` and implements `FileSystemSource`.
`entries()` collects and sorts entry names + metadata but does NOT open
child directory handles — each child is a lazy thunk that opens on demand.

Large files (>256 KiB) are memory-mapped for zero-copy reads.

## Write side (`DirSlotSink`)

`DirSlotSink` implements `FileSystemSink` for a slot (parent dir + name).
`create_directory()` creates the dir and returns a `DirDirSink` for
populating children. `create_regular_file()` creates and opens the file.
`create_symlink()` creates the symlink.
