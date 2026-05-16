# harmonia-store-path

Nix store path types, parsing, and validation.

This crate provides the fundamental store path types used throughout the
Harmonia workspace:

- `StorePath` — a parsed store path (hash + name, without store directory prefix)
- `StoreDir` — the store directory (e.g. `/nix/store`), used to resolve full paths
- `StorePathHash` — the 160-bit truncated hash portion of a store path
- `StorePathName` — the validated name portion of a store path
- `StorePathSet` — convenience alias for ordered set of store paths

It sits at the bottom of the store crate hierarchy, just above
`harmonia-utils-hash` and `harmonia-utils-base-encoding`.
