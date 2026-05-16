# harmonia-store-derivation

Nix derivation types and semantics.

This crate provides the pure, I/O-free types for Nix build planning:

- `derivation/` — Derivation (.drv) file format and semantics
- `derived_path/` — Paths derived from derivations (output references)
- `placeholder/` — Placeholder computation for derivation inputs
- `realisation/` — Store path realisation tracking

It depends on `harmonia-store-content-address` (for `ContentAddress` in
derivation outputs), `harmonia-store-path` (for `StorePath`, `StoreDir`),
and `harmonia-utils-signature` (for realisation signing).

## Feature Flags

- `test`: Enables proptest `Arbitrary` implementations for property-based testing
