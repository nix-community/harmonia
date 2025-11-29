# harmonia-utils-test

Proptest strategies and test macros for property-based testing.

## Overview

This crate provides proptest strategies for generating test data and assertion macros for property-based testing of Harmonia crates. It should only be used as a dev-dependency.

## Contents

- `arb_filename` / `arb_path` - Strategies for generating valid filenames and paths
- `arb_byte_string` - Strategy for generating arbitrary byte strings
- `arb_duration` / `arb_system_time` - Strategies for time values
- `pretty_prop_assert_eq!` - Assertion macro with pretty diff output
- `helpers::Union` - Weighted union of proptest strategies

## Example

```rust
use harmonia_utils_test::{arb_path, arb_byte_string, pretty_prop_assert_eq};
use proptest::prelude::*;

proptest! {
    #[test]
    fn roundtrip_path(path in arb_path()) {
        let encoded = encode(&path);
        let decoded = decode(&encoded)?;
        pretty_prop_assert_eq!(path, decoded);
    }

    #[test]
    fn roundtrip_bytes(data in arb_byte_string()) {
        let compressed = compress(&data);
        let decompressed = decompress(&compressed)?;
        pretty_prop_assert_eq!(data, decompressed);
    }
}
```

## Strategies

| Strategy | Generates |
|----------|-----------|
| `arb_filename()` | Valid filenames (no `.` or `..`) |
| `arb_path()` | Valid relative paths |
| `arb_file_component()` | Single path component |
| `arb_byte_string()` | Arbitrary `bytes::Bytes` |
| `arb_duration()` | `std::time::Duration` values |
| `arb_system_time()` | System time as duration |

## Key Characteristics

- Dev-dependency only (not needed at runtime)
- Generates valid data that satisfies invariants
- Pretty diff output on assertion failure
