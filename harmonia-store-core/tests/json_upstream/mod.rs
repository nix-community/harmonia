//! Tests that verify JSON serialization matches upstream Nix format
//!
//! These tests use JSON test data from the upstream Nix repository.

use serde::{Deserialize, Serialize};
use std::fmt::Debug;
use std::path::{Path, PathBuf};

fn upstream_test_data_path() -> PathBuf {
    // NIX_UPSTREAM_SRC environment variable should be set by the flake
    let nix_src =
        std::env::var("NIX_UPSTREAM_SRC").expect("NIX_UPSTREAM_SRC environment variable not set");
    PathBuf::from(nix_src).join("src")
}

fn libstore_test_data_path(relative_path: &str) -> PathBuf {
    upstream_test_data_path()
        .join("libstore-tests/data")
        .join(relative_path)
}

fn libutil_test_data_path(relative_path: &str) -> PathBuf {
    upstream_test_data_path()
        .join("libutil-tests/data")
        .join(relative_path)
}

/// Test reading (deserializing) from upstream Nix JSON format
fn test_upstream_json_from_json<T>(path: &Path, expected: &T)
where
    T: for<'de> Deserialize<'de> + PartialEq + Debug,
{
    let json_str = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {}", path.display(), e));

    let parsed: T = serde_json::from_str(&json_str)
        .unwrap_or_else(|e| panic!("Failed to parse {}: {}", path.display(), e));

    assert_eq!(parsed, *expected);
}

/// Test writing (serializing) to JSON and reading back (round-trip)
fn test_upstream_json_to_json<T>(path: &Path, value: &T)
where
    T: Serialize + for<'de> Deserialize<'de> + PartialEq + Debug,
{
    let json_str = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {}", path.display(), e));

    let json: serde_json::Value = serde_json::from_str(&json_str).unwrap();

    let serialized = serde_json::to_value(value).unwrap();
    assert_eq!(json, serialized);
}

/// Macro to generate both read and write tests for upstream JSON compatibility
#[macro_export]
macro_rules! test_upstream_json {
    ($test_name:ident, $path:expr, $value:expr) => {
        paste::paste! {
            #[test]
            fn [<$test_name _from_json>]() {
                $crate::test_upstream_json_from_json(&$path, &$value);
            }

            #[test]
            fn [<$test_name _to_json>]() {
                $crate::test_upstream_json_to_json(&$path, &$value);
            }
        }
    };
}

// Submodules organized by type
mod content_address;
mod derivation;
mod derivation_output;
mod derived_path;
mod hash;
mod outputs_spec;
mod realisation;
mod store_path;
