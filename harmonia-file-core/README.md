# harmonia-file-core

Generic file tree types and serde.

## Overview

This crate provides the core data types for representing file trees,
matching nix's `FileSystemObject` type hierarchy.

## Types

- `FileSystemObject<C, Ch>` — tagged enum: `Regular`, `Directory`, or `Symlink`
- `FileTree<C>` — recursive tree (newtype wrapping `FileSystemObject<C, Box<FileTree<C>>>`)
- `MemoryTree` — in-memory tree (`FileTree<Vec<u8>>`)

## Serde

All types derive `Serialize`/`Deserialize` with `#[serde(tag = "type")]`,
producing JSON matching `nix nar ls --json`.
