# harmonia-file-nar

NAR (Nix ARchive) format handling through `harmonia-file-io-pure` traits.

## Overview

This crate packs and unpacks NAR archives using the generic
`FileSystemSource` and `FileSystemSink` traits from `harmonia-file-io-pure`.
Any source (filesystem, memory tree, remote store) can be dumped to NAR,
and any NAR can be restored into any sink.

## Key functions

- **`dump_source`** — write a NAR archive from a `FileSystemSource` to an `AsyncWrite`
- **`restore_to_sink`** — parse a NAR archive and write to a `FileSystemSink`
- **`parse_nar_listing`** — parse a NAR archive into a `FileTree<NarFileInfo>` (JSON-compatible listing)
- **`NarByteStream`** — streaming NAR byte output from a filesystem path

## Example

```rust
use harmonia_file_core::{MemoryTreeSource, list_deep};
use harmonia_file_nar::{dump_source, restore_to_sink};

// Dump any FileSystemSource to NAR bytes
let mut nar = Vec::new();
dump_source(&source, &mut nar).await?;

// Restore NAR bytes into any FileSystemSink
restore_to_sink(reader, sink).await?;
```

## Design

- **Streaming**: Never requires entire NAR in memory
- **Trait-based**: Dump/restore go through `FileSystemSource`/`FileSystemSink`,
  not hardcoded to filesystem paths
- **Format-focused**: Only concerned with NAR archive structure
- **Composable**: Can be used independently of the daemon
