# Harmonia Store Structure

The crate layout follows the [hnix-store](https://github.com/haskell-nix/hnix-store)
layering model: pure store semantics are kept separate from effectful I/O so that
core logic can be tested in isolation and store backends can be swapped or
composed.

## Layers

```
┌──────────────────────────────────────────────────────┐
│  Application                                         │
│  harmonia-cache · harmonia-daemon · harmonia-client  │
└──────────────────────────────────────────────────────┘
                         ↓
┌──────────────────────────────────────────────────────┐
│  Protocol                                            │
│  harmonia-protocol · harmonia-protocol-derive        │
│  wire types, handshake, (de)serialization            │
└──────────────────────────────────────────────────────┘
                         ↓
┌────────────────────────────┬─────────────────────────┐
│  Format                    │  Database               │
│  harmonia-nar              │  harmonia-store-db      │
│  NAR pack/unpack, headers  │  SQLite store metadata  │
└────────────────────────────┴─────────────────────────┘
                         ↓
┌──────────────────────────────────────────────────────┐
│  Core (pure)                                         │
│  harmonia-store-core                                 │
│  store paths, derivations, references, signatures    │
│  no I/O, no async                                    │
└──────────────────────────────────────────────────────┘
                         ↓
┌──────────────────────────────────────────────────────┐
│  Utilities (harmonia-utils-*)                        │
│  io · base-encoding · hash · test                    │
│  protocol-specific building blocks, not Nix-specific │
└──────────────────────────────────────────────────────┘
```

## Crates

| Crate | Purpose |
|-------|---------|
| [harmonia-store-core](../../harmonia-store-core/README.md) | Store paths, derivations, signatures (pure) |
| [harmonia-store-aterm](../../harmonia-store-aterm/) | ATerm derivation parser |
| [harmonia-store-db](../../harmonia-store-db/README.md) | SQLite store metadata |
| [harmonia-nar](../../harmonia-nar/README.md) | NAR archive format |
| [harmonia-protocol](../../harmonia-protocol/README.md) | Daemon wire protocol |
| [harmonia-protocol-derive](../../harmonia-protocol-derive/README.md) | Derive macros for protocol types |
| [harmonia-daemon](../../harmonia-daemon/README.md) | Store daemon server |
| [harmonia-store-remote](../../harmonia-store-remote/README.md) | Daemon client library |
| [harmonia-ssh-store](../../harmonia-ssh-store/README.md) | Remote store over SSH (stub) |
| [harmonia-client](../../harmonia-client/README.md) | `harmonia` CLI binary (stub) |
| [harmonia-cache](../../harmonia-cache/README.md) | HTTP binary cache server |
| [harmonia-bench](../../harmonia-bench/) | Criterion benchmarks |
| [harmonia-utils-io](../../harmonia-utils-io/README.md) | Async byte streams, wire padding |
| [harmonia-utils-base-encoding](../../harmonia-utils-base-encoding/README.md) | Nix base32, hex, base64 |
| [harmonia-utils-hash](../../harmonia-utils-hash/README.md) | Hash types, algorithms, formatting |
| [harmonia-utils-test](../../harmonia-utils-test/README.md) | Proptest strategies (dev-only) |

The `harmonia-utils-*` crates are the inverse of `harmonia-store-core`: they
implement concrete protocols and encodings that Nix happens to use today
(nixbase32, NAR padding, hash algorithms) but contain no store semantics. They
could be reused outside Harmonia.

## Dependency Graph

```
utils-io      utils-base-encoding
   ↑                 ↑
   │           utils-hash
   │                 ↑
   ├───────────┬─────┤
   │           │     │
  nar    store-core  │   store-db
   ↑      ↑    ↑     │   (rusqlite only)
   │      │  store-aterm
   ├──────┴────┬─────┘
   │           │
   │       protocol
   │        ↑     ↑
   │        │     └── daemon (+ store-db)
   │   store-remote
   │        ↑
   └─────┬──┘
       cache
```

`harmonia-client`, `harmonia-ssh-store` and `harmonia-bench` currently have no
intra-workspace dependencies.

## Layer Rules

**Utilities** (`harmonia-utils-*`)
- No store semantics; protocol/encoding building blocks only.
- Minimal cross-dependencies between utils crates.
- I/O utils: async-first, bounded buffers, zero-copy where possible.
- Encoding/hash utils: pure, `const` where possible, tested against upstream
  vectors.

**Core** (`harmonia-store-core`)
- No I/O, no async, no `tokio`.
- Pure, deterministic functions returning `Result`; no panics.
- Stream-friendly (no unbounded buffers).

**Format** (`harmonia-nar`)
- Works against generic `AsyncRead`/`AsyncWrite`.
- Streaming; never requires the full input in memory.
- Knows nothing about derivations or signatures.

**Database** (`harmonia-store-db`)
- Schema matches Nix's `db.sqlite` exactly.
- System database opened read-only/immutable.
- Synchronous API; callers wrap in `spawn_blocking`.
- Tests use `:memory:`.

**Protocol** (`harmonia-protocol`)
- Versioned, backward-compatible with older protocol versions.
- Wire format documented on the types.
- Serialization round-trip tests; no I/O in the protocol types themselves.

**Application** (`daemon`, `client`, `cache`, `ssh-store`)
- Compose lower layers; do not reimplement core logic.
- This is where filesystem/network effects live.
- Structured logging, metrics, integration tests.

## Comparison with hnix-store

| hnix-store | Harmonia |
|------------|----------|
| hnix-store-core | harmonia-store-core |
| hnix-store-nar | harmonia-nar |
| hnix-store-json | harmonia-protocol (JSON lives in store-core due to orphan rules) |
| hnix-store-remote | harmonia-store-remote / harmonia-client |
| hnix-store-db | harmonia-store-db |
| hnix-store-readonly | (not yet split out) |
| (internal) | harmonia-utils-io / -base-encoding / -hash / -test |

Harmonia additionally ships a daemon server (`harmonia-daemon`); hnix-store is
client-side only.
