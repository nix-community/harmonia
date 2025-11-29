# Harmonia Store Structure

**Feature**: `001-nixrs-base`
**Inspiration**: [hnix-store](https://github.com/haskell-nix/hnix-store) layering model
**Date**: 2025-11-09

## Overview

This document describes the layered architecture adopted for Harmonia's store implementation, inspired by the hnix-store project's separation of concerns. The goal is to cleanly separate pure store semantics from effectful I/O operations, enabling better testability, modularity, and reuse.

## Architectural Principles

### 1. Separation of Semantics and Effects

**Core Insight** (from hnix-store):
> The store semantics provide the basic building blocks of Nix: content-addressed files and directories, the drv file format and the semantics for building drvs, tracking references of store paths, copying files between stores (or to/from caches), distributed builds, etc.

**Harmonia Application**:
- **Core/Pure layer**: Types, validation, computation (no IO)
- **Effectful layer**: Actual filesystem/network operations
- **Protocol layer**: Wire formats, serialization (independent of IO)

### 2. Composability

Different store implementations can be composed:
- **Readonly stores**: Defer to other implementations for reads, in-memory for mutations
- **Remote stores**: Talk to daemon over protocol
- **Local stores**: Direct filesystem access
- **Mock stores**: No IO at all (for testing)

### 3. Testability

Pure core layer enables:
- Unit tests without IO
- Property-based tests on semantics
- Deterministic test fixtures
- Fast test execution

## Layer Architecture

```
┌──────────────────────────────────────────────────────┐
│  Application Layer                                   │
│  - harmonia-cache (HTTP cache server)                │
│  - harmonia-daemon (Store daemon server)             │
│  - harmonia-client (Daemon client library)           │
│                                                      │
│  Role: Business logic, user-facing APIs              │
└──────────────────────────────────────────────────────┘
                         ↓
┌──────────────────────────────────────────────────────┐
│  Protocol Layer                                      │
│  - harmonia-protocol                                 │
│    · Wire protocol types (handshake, operations)     │
│    · Serialization/deserialization                   │
│    · Derive macros for protocol messages             │
│                                                      │
│  Role: Define how store operations are communicated  │
└──────────────────────────────────────────────────────┘
                         ↓
┌────────────────────────────┬──────────────────────────────┐
│  Format Layer              │  Database Layer              │
│  - harmonia-nar            │  - harmonia-store-db         │
│    · NAR packing/unpacking │    · SQLite store metadata   │
│    · NAR header parsing    │    · ValidPaths, Refs        │
│    · Streaming NAR ops     │    · DerivationOutputs       │
│                            │    · Realisations (CA)       │
│  Role: Archive format      │  Role: Store metadata access │
└────────────────────────────┴──────────────────────────────┘
                         ↓
┌──────────────────────────────────────────────────────────────────────────────┐
│  Core Layer (Pure Semantics)                                                 │
│  - harmonia-store-core                                                       │
│    · Store path types and validation                                         │
│    · Derivation parsing and building                                         │
│    · Reference graph computation                                             │
│    · Signature verification                                                  │
│                                                                              │
│  Role: WHAT operations mean (no IO, no async, pure functions)                │
└──────────────────────────────────────────────────────────────────────────────┘
                         ↓
┌──────────────────────────────────────────────────────────────────────────────┐
│  Utilities Layer (harmonia-utils-*)                                          │
│  ┌─────────────────┬──────────────────────┬─────────────────┬──────────────┐ │
│  │ harmonia-utils- │ harmonia-utils-      │ harmonia-utils- │ harmonia-    │ │
│  │ io              │ base-encoding        │ hash            │ utils-test   │ │
│  │                 │                      │                 │              │ │
│  │ · Async byte    │ · Nix base32         │ · Hash types    │ · Proptest   │ │
│  │   streams       │ · Hex, Base64        │ · Algorithms    │   strategies │ │
│  │ · BytesReader   │ · Base enum          │ · HashSink      │ · Test       │ │
│  │ · Wire padding  │                      │ · Formatting    │   macros     │ │
│  └─────────────────┴──────────────────────┴─────────────────┴──────────────┘ │
│                                                                              │
│  Role: Reusable building blocks (protocol-specific, not Nix-specific)        │
└──────────────────────────────────────────────────────────────────────────────┘
```

## Crates

### Main Crates

| Crate | Purpose | Details |
|-------|---------|---------|
| [harmonia-store-core](../../harmonia-store-core/README.md) | Pure store semantics | Store paths, derivations, signatures |
| [harmonia-store-db](../../harmonia-store-db/README.md) | SQLite database | Store metadata access |
| [harmonia-nar](../../harmonia-nar/README.md) | NAR format | Archive packing/unpacking |
| [harmonia-protocol](../../harmonia-protocol/README.md) | Wire protocol | Daemon communication |
| [harmonia-protocol-derive](../../harmonia-protocol-derive/README.md) | Derive macros | Protocol serialization |
| [harmonia-daemon](../../harmonia-daemon/README.md) | Daemon server | Store operations server |
| [harmonia-store-remote](../../harmonia-store-remote/README.md) | Daemon client | Remote store access |
| [harmonia-client](../../harmonia-client/README.md) | CLI wrapper | Nix command wrapper |
| [harmonia-cache](../../harmonia-cache/README.md) | Binary cache | HTTP cache server |
| [harmonia-ssh-store](../../harmonia-ssh-store/README.md) | SSH store | Remote store via SSH |

### Utility Crates (`harmonia-utils-*`)

| Crate | Purpose | Details |
|-------|---------|---------|
| [harmonia-utils-io](../../harmonia-utils-io/README.md) | Async I/O | Streaming, buffering |
| [harmonia-utils-base-encoding](../../harmonia-utils-base-encoding/README.md) | Base encodings | Nix base32, hex, base64 |
| [harmonia-utils-hash](../../harmonia-utils-hash/README.md) | Hash utilities | Algorithms, formatting |
| [harmonia-utils-test](../../harmonia-utils-test/README.md) | Test utilities | Proptest strategies |

**Purpose**:
Somewhat the opposite of harmonia-store-core.
Reusable building blocks, very much geared towards specific protocols that Nix happens to use today (e.g. NAR).
It is easy to imagine other versions of Nix not making these specific choices (e.g. different protocols, different hash algorithms, etc.)
Also while these crates are implementation-specific, they are somewhat purpose-/interface-agnostic --- nothing in here is really "Nix-specific", nothing in here is "business logic", except for in the most mundane ways (like choices of hash algorithms, and nixbase32 having a different alphabet).

**Key Characteristics** (all utils crates):
- Foundation for higher-level crates
- Independent of harmonia-store-core (no store semantics)
- Async-first design with bounded memory usage (for I/O crates)
- Pure functions, no I/O (for encoding/hash crates)

## Benefits of This Structure

### 1. Independent Testing

**Core Layer** (harmonia-store-core):
- Unit tests with pure functions
- Property-based tests (proptest) for hash/path operations
- No test fixtures needed for filesystem

**Format Layer** (harmonia-nar):
- Test with in-memory buffers
- Fixtures are just byte arrays
- Streaming tests with mock IO

**Protocol Layer** (harmonia-protocol):
- Mock protocol messages
- Test serialization round-trips
- Protocol compatibility tests

**Application Layer**:
- Integration tests with real daemon
- End-to-end tests with actual store

### 2. Modularity and Reuse

**Example 1**: Different store backends
```rust
// Local filesystem store
let store = LocalStore::new("/nix/store");

// Remote daemon store
let store = RemoteStore::connect("unix:///nix/var/nix/daemon-socket/socket");

// Both implement the same Store trait from harmonia-store-core
```

**Example 2**: NAR operations outside daemon
```rust
// harmonia-cache can use harmonia-nar directly for serving
// without going through daemon
let nar_stream = pack_nar(&store_path).await?;
response.send_stream(nar_stream).await?;
```

### 3. Clear Dependency Graph

```
┌──────────────────────────────────────────────────────────────────────────────┐
│  Utilities Layer (no harmonia deps, except within utils)                     │
│                                                                              │
│  harmonia-utils-io          harmonia-utils-base-encoding                     │
│  (no deps)                  (no deps)                                        │
│       ↑                            ↑                                         │
│       │                            │                                         │
│       │                     harmonia-utils-hash                              │
│       │                     (depends on: base-encoding)                      │
│       │                            ↑                                         │
│       │                            │                                         │
│  harmonia-utils-test (dev only, depends on: proptest)                        │
└──────────────────────────────────────────────────────────────────────────────┘
         ↑                            ↑
         │                            │
         ├────────────────────────────┤
         │                            │
    harmonia-store-core          harmonia-nar
    (depends on:                 (depends on: io)
     base-encoding, hash)
         ↑                            ↑
         │                            │
         ├────────────────────────────┤
         │                            │
    harmonia-store-db         harmonia-protocol
    (depends on:              (depends on: io, hash,
     store-core)               store-core, nar)
                                      ↑
                                      │
         ┌────────────────────────────┼────────────────────────────┐
         │                            │                            │
  harmonia-daemon             harmonia-store-remote         harmonia-ssh-store
  (depends on:                (depends on:                  (depends on:
   io, hash, store-core,       io, store-core,               store-remote)
   nar, protocol, db)          protocol, nar)
         │                            │
         └────────────┬───────────────┘
                      │
               harmonia-cache
               (depends on:
                store-remote,
                nar for direct serving)
```

### 4. Performance Optimization

**Streaming at every layer**:
- Core: Stream-friendly hash computation
- Format: Streaming NAR pack/unpack
- Protocol: Streaming wire protocol
- Application: End-to-end streaming

**Example**: Serving a NAR from cache
```rust
// No intermediate buffering needed
let nar_hash = compute_nar_hash_streaming(&path).await?;  // harmonia-store-core
let nar_stream = pack_nar_streaming(&path).await?;        // harmonia-nar
response.send_stream(nar_stream).await?;                   // harmonia-cache
```

## Comparison with hnix-store

| hnix-store | Harmonia | Notes |
|------------|----------|-------|
| (internal) | harmonia-utils-io | Async I/O primitives (extracted for reuse) |
| (none) | harmonia-utils-base-encoding | Nix base32/hex/base64 encoding (standalone crate) |
| (none) | harmonia-utils-hash | Hash types and algorithms (standalone crate) |
| (internal) | harmonia-utils-test | Test utilities (proptest strategies) |
| hnix-store-core | harmonia-store-core | Pure semantics, types |
| hnix-store-nar | harmonia-nar | Archive format |
| hnix-store-json | harmonia-protocol | Wire protocol (not just JSON). (JSON is actually in harmonia-store-core, because Rust doesn't support orphan instances.) |
| hnix-store-remote | harmonia-client | Daemon client |
| hnix-store-db | harmonia-store-db | SQLite DB for store metadata |
| hnix-store-readonly | (future) | Could add as separate crate |

**Key Differences**:
- Harmonia has a separate `harmonia-daemon` server implementation, whereas hnix-store focuses on client-side store abstractions.
- Harmonia extracts base encoding and hash utilities into standalone crates (`harmonia-utils-base-encoding`, `harmonia-utils-hash`) that can be reused by other projects needing Nix-compatible encoding/hashing.

## Implementation Guidelines

### Utilities Layer Rules (harmonia-utils-*)

1. **No store semantics**: Generic utilities only, nothing Nix-specific in business logic
2. **Minimal dependencies**: Utils crates should have minimal deps on each other
3. **Protocol-specific, not Nix-specific**: Implementation choices (hash algos, encodings) not business logic
4. **Composable**: Traits and utilities that work together
5. **Well-tested**: Property-based tests and known test vectors

*For I/O utilities specifically*:
- Async-first: Designed for non-blocking I/O
- Bounded memory: Configurable buffer sizes, no unbounded growth
- Zero-copy where possible: Minimize data copying

*For encoding/hash utilities specifically*:
- Pure functions: No I/O, deterministic
- const where possible: Compile-time evaluation

### Core Layer Rules

1. **No IO**: No filesystem access, no network
2. **No async**: All operations are synchronous computations
3. **Pure functions**: Same input → same output
4. **Explicit errors**: Use Result types, no panics
5. **Memory-bounded**: Stream-friendly, no unbounded buffers

### Format Layer Rules

1. **IO-agnostic**: Work with traits (AsyncRead/AsyncWrite)
2. **Streaming**: Don't require entire input in memory
3. **Format-focused**: Only concerned with archive structure
4. **No store semantics**: Don't know about derivations, signatures, etc.

### Database Layer Rules

1. **Schema-compatible**: Match Nix's db.sqlite schema exactly
2. **Read-only default**: System database opens in immutable mode
3. **In-memory testing**: Support `:memory:` for fast tests
4. **No async**: SQLite operations are synchronous (wrap in spawn_blocking)

### Protocol Layer Rules

1. **Versioned**: Support protocol version negotiation
2. **Backward-compatible**: Handle older protocol versions
3. **Well-specified**: Document wire format
4. **Efficient serialization**: Minimize copies

### Application Layer Rules

1. **Use lower layers**: Don't reimplement core logic
2. **Add IO effects**: This is where filesystem/network happens
3. **Observability**: Logging, metrics, tracing
4. **Error handling**: Convert lower-layer errors to user-facing errors

## Review Checklist

When reviewing code, ensure:

**Core layer** (harmonia-store-core):
- [ ] No I/O, should be all pure.
- [ ] Hardly any `std::io`, therefore
- [ ] No `use tokio::fs` or network imports
- [ ] No `async` in public API
- [ ] All functions are deterministic
- [ ] Comprehensive unit tests
- [ ] Property-based tests for core operations

**Utilities layer** (harmonia-utils-*):

*harmonia-utils-io*:
- [ ] No store-specific types or logic
- [ ] Uses generic async I/O traits
- [ ] Buffer sizes are configurable
- [ ] Memory usage is bounded
- [ ] Tests use mock I/O (tokio-test)

*harmonia-utils-base-encoding*:
- [ ] No dependencies on other harmonia crates
- [ ] Pure functions only
- [ ] Comprehensive test vectors (from Nix/upstream)
- [ ] Property-based roundtrip tests

*harmonia-utils-hash*:
- [ ] Only depends on harmonia-utils-base-encoding
- [ ] Pure functions (except HashSink which is async)
- [ ] All hash algorithms tested against known vectors
- [ ] Property-based tests for formatting roundtrips

*harmonia-utils-test*:
- [ ] Only used as dev-dependency
- [ ] Strategies generate valid data
- [ ] No runtime dependencies on test frameworks

**Format layer** (harmonia-nar):
- [ ] Uses generic AsyncRead/AsyncWrite traits
- [ ] Streaming-friendly (bounded memory usage)
- [ ] Independent of store semantics
- [ ] Tests use in-memory buffers

**Database layer** (harmonia-store-db):
- [ ] Schema matches Nix's schema.sql exactly
- [ ] System DB opens read-only with immutable flag
- [ ] Tests use in-memory database
- [ ] No async in public API (callers wrap in spawn_blocking)

**Protocol layer** (harmonia-protocol):
- [ ] Protocol messages well-documented
- [ ] Version compatibility handled
- [ ] Serialization round-trip tests
- [ ] No IO in protocol types themselves

**Application layer** (daemon, client, cache):
- [ ] Uses lower layers correctly
- [ ] Adds structured logging
- [ ] Emits Prometheus metrics
- [ ] Integration tests cover IO paths

## References

- hnix-store: https://github.com/haskell-nix/hnix-store
- hnix-store README: `/home/joerg/git/hnix-store/README.md`
- Nix.rs upstream: `/home/joerg/git/Nix.rs`
- Feature spec: `specs/001-nixrs-base/spec.md`
- Research notes: `specs/001-nixrs-base/research.md`

## Next Steps

1. Implement core layer first (harmonia-store-core)
2. Add format layer (harmonia-nar)
3. Define protocol (harmonia-protocol)
4. Build daemon (harmonia-daemon)
5. Implement client (harmonia-client)
6. Integrate into cache (harmonia-cache)
