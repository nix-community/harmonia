# harmonia-utils-hash

Cryptographic hash utilities for content addressing.

## Overview

This crate provides hash types and algorithms used for content addressing in Nix. It supports MD5, SHA1, SHA256, and SHA512, with various output formats (hex, Nix base32, base64, SRI). It is a standalone crate that can be used by any project needing Nix-compatible hashing.

## Contents

- `Hash` - Generic hash type supporting MD5, SHA1, SHA256, SHA512
- `Algorithm` - Hash algorithm enum with size and digest operations
- `Sha256` / `NarHash` - Specialized hash types for common use cases
- `Context` - Multi-step (Init-Update-Finish) digest calculation
- `HashSink` - Async writer that computes hash of written data
- `fmt` - Hash formatting (Base16, Base32, Base64, SRI)

## Example

```rust
use harmonia_utils_hash::{Algorithm, Context, Hash};

// One-shot hash computation
let hash = Algorithm::SHA256.digest(b"hello, world");

// Multi-step hashing
let mut ctx = Context::new(Algorithm::SHA256);
ctx.update("hello");
ctx.update(", world");
let hash = ctx.finish();

// Hash formatting
let base32 = hash.as_base32().to_string();  // "1b8m03r63zqh..."
let sri = hash.sri().to_string();           // "sha256-ungWv48B..."
let hex = hash.as_base16().to_string();     // "ba7816bf8f01..."

// Async hashing with HashSink
use tokio::io;
let mut sink = harmonia_utils_hash::HashSink::new(Algorithm::SHA256);
io::copy(&mut reader, &mut sink).await?;
let (size, hash) = sink.finish();
```

## Supported Algorithms

| Algorithm | Size (bytes) | Base16 | Base32 | Base64 |
|-----------|--------------|--------|--------|--------|
| MD5       | 16           | 32     | 26     | 24     |
| SHA1      | 20           | 40     | 32     | 28     |
| SHA256    | 32           | 64     | 52     | 44     |
| SHA512    | 64           | 128    | 103    | 88     |

## Key Characteristics

- Pure functions (hash computation is deterministic)
- Multiple output formats: Base16 (hex), Nix Base32, Base64, SRI
- Only depends on harmonia-utils-base-encoding
