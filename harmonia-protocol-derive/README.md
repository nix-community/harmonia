# harmonia-protocol-derive

Derive macros for Nix daemon protocol serialization.

## Overview

This crate provides derive macros for implementing `NixDeserialize` and `NixSerialize` traits with less boilerplate. These macros generate the serialization code needed for types to be sent over the Nix daemon wire protocol.

## Contents

- **`#[derive(NixDeserialize)]`** - Derive deserialization from wire format
- **`#[derive(NixSerialize)]`** - Derive serialization to wire format
- **`nix_deserialize_remote!`** - Implement deserialization for external types
- **`nix_serialize_remote!`** - Implement serialization for external types

## Example

```rust
use harmonia_protocol_derive::{NixDeserialize, NixSerialize};

#[derive(NixDeserialize, NixSerialize)]
struct MyMessage {
    id: u64,
    #[nix(version = "..20")]
    legacy_field: String,
    #[nix(version = "20..")]
    new_field: Vec<u8>,
}

#[derive(NixDeserialize)]
#[nix(from_str)]
struct ParsedValue(String);
```

## Attributes

### Container Attributes

| Attribute | Description |
|-----------|-------------|
| `#[nix(from_str)]` | Deserialize via `FromStr::from_str` |
| `#[nix(from_store_dir_str)]` | Deserialize via `FromStoreDirStr` |
| `#[nix(from = "Type")]` | Deserialize via `From<Type>` |
| `#[nix(try_from = "Type")]` | Deserialize via `TryFrom<Type>` |
| `#[nix(into = "Type")]` | Serialize via `Into<Type>` |
| `#[nix(try_into = "Type")]` | Serialize via `TryInto<Type>` |
| `#[nix(display)]` | Serialize via `Display` |
| `#[nix(store_dir_display)]` | Serialize via `StoreDirDisplay` |
| `#[nix(crate = "...")]` | Specify crate path |

### Field/Variant Attributes

| Attribute | Description |
|-----------|-------------|
| `#[nix(version = "range")]` | Include only in specified protocol versions |
| `#[nix(default)]` | Use `Default::default()` when field is skipped |
| `#[nix(default = "path")]` | Call function to get default value |

## Key Characteristics

- **Version-aware**: Fields can be conditionally included based on protocol version
- **Flexible conversion**: Support multiple conversion strategies
- **Proc-macro crate**: Used as a build dependency
