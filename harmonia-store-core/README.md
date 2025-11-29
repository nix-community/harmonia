# harmonia-store-core (Core)

**Purpose**: Pure store semantics for Nix, agnostic to I/O and implementation strategy.

## Overview

This crate provides the fundamental types and pure computation logic for working with the Nix store. It is intentionally I/O-free - all operations are pure functions that operate on values, enabling easy testing and composition.

This is the "business logic" of Nix, pure and simple. It should be usable with a wide variety of implementation strategies, not forcing any decisions. It should also be widely usable by other tools which need to engage with Nix (e.g. tools that create dynamic derivations from other build systems' build plans).

## Contents (from Nix.rs):

- `store_path/` - Store path parsing, validation, manipulation
- `derivation/` - Derivation (.drv) file format and semantics
- `derived_path/` - Paths derived from derivations (built outputs)
- `signature/` - Cryptographic signatures for store paths
- `realisation/` - Store path realisation tracking
- `placeholder/` - Placeholder computation for derivation inputs
- `log/` - Build log types

## Key Characteristics

- No `async`, no filesystem access, no network
- All operations are pure computations
- Can be tested without IO
- Can be compiled to WASM

## Example

```rust
use harmonia_store_core::store_path::{StorePath, StoreDir};
use harmonia_store_core::derivation::Derivation;

// Pure computation - no IO
pub fn parse_store_path(path: &str) -> Result<StorePath, ParseError>;
pub fn compute_hash(content: &[u8], hash_type: HashType) -> Hash;
pub fn verify_signature(path: &StorePath, sig: &Signature) -> bool;
```

## Feature Flags

- `test`: Enables proptest `Arbitrary` implementations for property-based testing
