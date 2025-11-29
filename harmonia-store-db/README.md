# harmonia-store-db (Database)

**Purpose**: SQLite database interface for Nix store metadata

## Overview

This crate provides read and write access to the Nix store's SQLite database, enabling queries for store path metadata, references, and derivation outputs. It is the Database Layer in Harmonia's store architecture.

New implementation (inspired by hnix-store-db).

## Contents

New implementation (inspired by hnix-store-db)

- Full Nix schema support (ValidPaths, Refs, DerivationOutputs, Realisations)
- Read-only system database access (immutable mode)
- In-memory database for testing
- Write operations for testing and local store management

## Key Characteristics

Direct metadata access:
- Bypasses daemon for metadata queries
- Useful for direct store inspection
- Schema matches Nix's db.sqlite exactly

## Example

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
