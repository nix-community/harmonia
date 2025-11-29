# harmonia-utils-io

Reusable async I/O building blocks for streaming protocols.

## Overview

This crate provides async I/O primitives used throughout Harmonia for reading and writing byte streams. It is designed for protocols like NAR that require buffered, streaming I/O with bounded memory usage.

## Contents

- `AsyncBytesRead` - Async trait for reading byte streams with buffering
- `BytesReader` - Buffered async byte reader with configurable buffer sizes
- `Lending` / `LentReader` - Reader lending for composable stream processing
- `DrainInto` - Drain remaining bytes from a reader
- `TeeWriter` - Write to two destinations simultaneously
- `wire` - Wire protocol primitives (padding, alignment, zero bytes)

## Example

```rust
use harmonia_utils_io::{AsyncBytesRead, BytesReader, wire};

// Async byte reading with buffering
pub trait AsyncBytesRead: AsyncRead {
    fn poll_fill_buf(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<Bytes>>;
    fn consume(self: Pin<&mut Self>, amt: usize);
}

// Wire protocol utilities
pub mod wire {
    pub const ZEROS: [u8; 8] = [0u8; 8];
    pub const fn calc_padding(len: u64) -> usize;
    pub const fn calc_aligned(len: u64) -> u64;
}
```

## Key Characteristics

- Async-first design with tokio
- Bounded memory usage with configurable buffer sizes
- Zero-copy where possible using `bytes::Bytes`
