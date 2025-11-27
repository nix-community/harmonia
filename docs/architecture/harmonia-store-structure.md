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
│  Application Layer                                    │
│  - harmonia-cache (HTTP cache server)                │
│  - harmonia-daemon (Store daemon server)             │
│  - harmonia-client (Daemon client library)           │
│                                                       │
│  Role: Business logic, user-facing APIs              │
└──────────────────────────────────────────────────────┘
                         ↓
┌──────────────────────────────────────────────────────┐
│  Protocol Layer                                       │
│  - harmonia-protocol                                  │
│    · Wire protocol types (handshake, operations)     │
│    · Serialization/deserialization                   │
│    · Derive macros for protocol messages             │
│                                                       │
│  Role: Define how store operations are communicated  │
└──────────────────────────────────────────────────────┘
                         ↓
┌────────────────────────────┬─────────────────────────────┐
│  Format Layer              │  Database Layer              │
│  - harmonia-nar            │  - harmonia-store-db         │
│    · NAR packing/unpacking │    · SQLite store metadata   │
│    · NAR header parsing    │    · ValidPaths, Refs        │
│    · Streaming NAR ops     │    · DerivationOutputs       │
│                            │    · Realisations (CA)       │
│  Role: Archive format      │  Role: Store metadata access │
└────────────────────────────┴─────────────────────────────┘
                         ↓
┌──────────────────────────────────────────────────────┐
│  Core Layer (Pure Semantics)                         │
│  - harmonia-store-core                               │
│    · Store path types and validation                 │
│    · Content addressing (hashes, hash types)         │
│    · Derivation parsing and building                 │
│    · Reference graph computation                     │
│    · Signature verification                          │
│                                                       │
│  Role: Define WHAT operations mean, not HOW to do IO │
└──────────────────────────────────────────────────────┘
```

## Crate Responsibilities

### harmonia-store-core (Core)

**Purpose**: Pure store semantics, agnostic to IO

**Contents** (from Nix.rs):
- `hash/` - Hash types, algorithms, content addressing
- `store_path/` - Store path parsing, validation, manipulation
- `derivation/` - Derivation (.drv) file format and semantics
- `signature/` - Cryptographic signatures for store paths
- `realisation/` - Store path realisation tracking

**Key Characteristic**: No `async`, no filesystem access, no network
- All operations are pure computations
- Can be tested without IO
- Can be compiled to WASM

**Example API**:
```rust
// Pure computation - no IO
pub fn parse_store_path(path: &str) -> Result<StorePath, ParseError>;
pub fn compute_hash(content: &[u8], hash_type: HashType) -> Hash;
pub fn verify_signature(path: &StorePath, sig: &Signature) -> bool;
```

### harmonia-store-db (Database)

**Purpose**: SQLite database interface for Nix store metadata

**Contents**: New implementation (inspired by hnix-store-db)
- Full Nix schema support (ValidPaths, Refs, DerivationOutputs, Realisations)
- Read-only system database access (immutable mode)
- In-memory database for testing
- Write operations for testing and local store management

**Key Characteristic**: Direct metadata access
- Bypasses daemon for metadata queries
- Useful for direct store inspection
- Schema matches Nix's db.sqlite exactly

**Example API**:
```rust
// Open system database (read-only)
let db = StoreDb::open_system()?;

// Query path info with references
let info = db.query_path_info("/nix/store/...")?;
let refs = db.query_references("/nix/store/...")?;
let derivers = db.query_valid_derivers("/nix/store/...")?;

// In-memory for testing
let db = StoreDb::open_memory()?;
db.register_valid_path(&params)?;
```

### harmonia-nar (Format)

**Purpose**: NAR archive format handling

**Contents** (from Nix.rs):
- `archive/` - NAR packing/unpacking logic
- NAR header parsing
- Streaming NAR operations

**Key Characteristic**: Format-specific, but IO-agnostic
- Can work with any IO source/sink
- Reusable across different store implementations
- Streaming-friendly (doesn't require entire NAR in memory)

**Example API**:
```rust
// Takes any AsyncRead, returns parsed NAR
pub async fn unpack_nar<R: AsyncRead>(reader: R) -> Result<NarContents, NarError>;

// Takes contents, writes to any AsyncWrite
pub async fn pack_nar<W: AsyncWrite>(contents: &Path, writer: W) -> Result<(), NarError>;
```

### harmonia-protocol (Protocol)

**Purpose**: Daemon wire protocol definition

**Contents** (from Nix.rs):
- `wire/` - Protocol message types
- Serialization/deserialization for protocol
- Derive macros for protocol messages (from nixrs-derive)

**Key Characteristic**: Protocol-focused
- Defines the contract between client and daemon
- Version negotiation
- Operation encoding/decoding

**Example API**:
```rust
#[derive(NixProtocol)]
pub enum Operation {
    QueryValidPaths { paths: Vec<StorePath> },
    QueryPathInfo { path: StorePath },
    NarFromPath { path: StorePath },
    // ...
}

pub trait ProtocolCodec {
    async fn read_operation<R: AsyncRead>(&mut self, reader: R) -> Result<Operation>;
    async fn write_operation<W: AsyncWrite>(&mut self, writer: W, op: &Operation) -> Result<()>;
}
```

### harmonia-daemon (Implementation)

**Purpose**: Daemon server implementation

**Contents** (from Nix.rs):
- `daemon/` - Server logic, socket handling
- Store operations implementation
- Worker threads/connection management

**Key Characteristic**: Ties everything together
- Uses harmonia-store-core for semantics
- Uses harmonia-nar for archive operations
- Uses harmonia-protocol for communication
- Adds IO effects (filesystem, sockets)

**Example API**:
```rust
pub struct Daemon {
    store: Store,
    config: DaemonConfig,
}

impl Daemon {
    pub async fn serve(&self, listener: UnixListener) -> Result<()> {
        // Accept connections, handle protocol operations
    }
}
```

### harmonia-client (Implementation)

**Purpose**: Daemon client library with connection pooling

**Contents**: New implementation for Harmonia
- Protocol client using harmonia-protocol types
- Connection pool with queue management
- Retry logic and error handling
- Metrics and observability hooks

**Key Characteristic**: Reusable client library
- Built-in connection pooling (no separate pool crate needed)
- Typed errors
- Async-first API

**Example API**:
```rust
pub struct Client {
    pool: ConnectionPool,
    config: ClientConfig,
}

impl Client {
    pub async fn query_path_info(&self, path: &StorePath) -> Result<PathInfo>;
    pub async fn nar_from_path(&self, path: &StorePath) -> Result<impl AsyncRead>;

    // Pool management
    pub fn pool_metrics(&self) -> PoolMetrics;
}
```

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
harmonia-store-core (no dependencies on other harmonia crates)
         ↑
         │
    harmonia-nar (depends on: store-core for types)
         ↑
         │
 harmonia-protocol (depends on: store-core for types)
         ↑
         ├────────────────┐
         │                │
  harmonia-daemon   harmonia-client
  (depends on:      (depends on:
   store-core,     store-core,
   nar,              protocol)
   protocol)
         │                │
         └────────┬───────┘
                  │
           harmonia-cache
           (depends on:
            client,
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
| hnix-store-core | harmonia-store-core | Pure semantics, types |
| hnix-store-nar | harmonia-nar | Archive format |
| hnix-store-json | harmonia-protocol | Wire protocol (not just JSON) |
| hnix-store-remote | harmonia-client | Daemon client |
| hnix-store-db | harmonia-store-db | SQLite DB for store metadata |
| hnix-store-readonly | (future) | Could add as separate crate |

**Key Difference**: Harmonia has a separate `harmonia-daemon` server implementation, whereas hnix-store focuses on client-side store abstractions.

## Implementation Guidelines

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
- [ ] No `use tokio::fs` or network imports
- [ ] No `async` in public API
- [ ] All functions are deterministic
- [ ] Comprehensive unit tests
- [ ] Property-based tests for core operations

**Format layer** (harmonia-nar):
- [ ] Uses generic AsyncRead/AsyncWrite traits
- [ ] Streaming-friendly (bounded memory usage)
- [ ] Independent of store semantics
- [ ] Tests use in-memory buffers

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
