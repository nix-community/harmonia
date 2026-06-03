# harmonia-file-io-pure

Async file tree IO traits, in-memory implementations, and listing functions.

## Traits

- **`FileSystemSource`** — async read-side interface. A node in a file
  tree; navigate to children with `open()`, iterate with `entries()`.
  Entries yield lazy `ChildThunk` futures so directory handles are only
  opened on demand.

- **`FileSystemSink`** — async write-side interface. Creates a single
  node (file, dir, or symlink). `create_directory()` returns a
  `DirectorySink` for populating children one level at a time.

- **`RegularFileSink`** — `AsyncWrite` for streaming file contents.

Mirrors nix's `SourceAccessor` / `FileSystemObjectSink` architecture
([NixOS/nix#15392](https://github.com/NixOS/nix/pull/15392)) in Rust
with async traits.

## In-memory implementation

`MemoryTreeSource` and `MemoryTreeBuilder` provide a pure in-memory
implementation for testing and for building trees from parsed NARs.

## Listing

- `list_deep` — fully recursive listing from a `FileSystemSource`
- `list_shallow` — one-level listing with `Opaque` children
