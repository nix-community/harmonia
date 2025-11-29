# harmonia-nar (Format)

**Purpose**: NAR archive format handling

## Overview

This crate provides functionality for packing and unpacking NAR archives, the archive format used by Nix for representing store paths as byte streams. It is the Format Layer in Harmonia's store architecture.

## Contents (from Nix.rs):

- `archive/` - NAR packing/unpacking logic
- NAR header parsing
- Streaming NAR operations

## Key Characteristics

Format-specific, but IO-agnostic:
- Can work with any IO source/sink
- Reusable across different store implementations
- Streaming-friendly (doesn't require entire NAR in memory)

## Example

```rust
// Takes any AsyncRead, returns parsed NAR
pub async fn unpack_nar<R: AsyncRead>(reader: R) -> Result<NarContents, NarError>;

// Takes contents, writes to any AsyncWrite
pub async fn pack_nar<W: AsyncWrite>(contents: &Path, writer: W) -> Result<(), NarError>;
```
