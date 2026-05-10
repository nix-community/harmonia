# harmonia-file-nar

NAR (Nix ARchive) format handling.

## Overview

This crate packs and unpacks NAR archives, the archive format used by
Nix for representing store paths as byte streams.

## Key types

- **`NarDumper`** — stream that walks a filesystem path and produces `NarEvent`s
- **`NarRestorer`** — sink that restores `NarEvent`s to a filesystem path
- **`NarByteStream`** — streaming NAR byte output for HTTP serving
- **`NarParser`** — parse NAR bytes into `NarEvent`s
- **`parse_nar_listing`** — parse NAR bytes into a `FileTree<NarFileInfo>` listing

## Design

- **Streaming**: Never requires entire NAR in memory
- **Async**: Built on tokio
- **Format-focused**: Only concerned with NAR archive structure
- **Composable**: Can be used independently of the daemon
