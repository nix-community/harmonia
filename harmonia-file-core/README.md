# harmonia-file-core

Generic file tree types, async traits, and listing functions.

## Overview

This crate provides the core abstractions for working with file trees,
mirroring nix's `SourceAccessor` / `FileSystemObjectSink` architecture
([NixOS/nix#15392](https://github.com/NixOS/nix/pull/15392)) in Rust
with async traits.

## Traits

- **`FileSystemSource`** — async read-side interface. A node in a file
  tree; navigate to children with `open()`, iterate with `entries()`.
  Entries yield lazy `ChildThunk` futures so directory handles are only
  opened on demand.

- **`FileSystemSink`** — async write-side interface. Creates a single
  node (file, dir, or symlink). `create_directory()` returns a
  `DirectorySink` for populating children one level at a time.

- **`RegularFileSink`** — `AsyncWrite` for streaming file contents.

## Types

- `FileSystemObject<C, Ch>` — tagged enum: `Regular`, `Directory`, or `Symlink`
- `FileTree<C>` — recursive tree (newtype wrapping `FileSystemObject<C, Box<FileTree<C>>>`)
- `ShallowTree<C>` — one-level tree with `Opaque` children
- `MemoryTree` — in-memory tree (`FileTree<Vec<u8>>`)

## Serde

All types derive `Serialize`/`Deserialize` with `#[serde(tag = "type")]`,
producing JSON matching `nix nar ls --json`.

## In-memory implementation

`MemoryTreeSource` and `MemoryTreeBuilder` provide a pure in-memory
implementation for testing and for building trees from parsed NARs.
