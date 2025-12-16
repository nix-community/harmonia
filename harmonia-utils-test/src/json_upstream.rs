//! Test infrastructure for upstream Nix JSON format compatibility.
//!
//! This module provides utilities for testing that types serialize and deserialize
//! correctly according to the upstream Nix JSON format.

use serde::{Deserialize, Serialize};
use std::fmt::Debug;
use std::path::{Path, PathBuf};

/// Get the path to upstream Nix test data.
///
/// This reads the `NIX_UPSTREAM_SRC` environment variable which should be set
/// by the flake to point to the Nix source tree.
pub fn upstream_test_data_path() -> PathBuf {
    let nix_src =
        std::env::var("NIX_UPSTREAM_SRC").expect("NIX_UPSTREAM_SRC environment variable not set");
    PathBuf::from(nix_src).join("src")
}

/// Get the path to libstore test data.
pub fn libstore_test_data_path(relative_path: &str) -> PathBuf {
    upstream_test_data_path()
        .join("libstore-tests/data")
        .join(relative_path)
}

/// Get the path to libutil test data.
pub fn libutil_test_data_path(relative_path: &str) -> PathBuf {
    upstream_test_data_path()
        .join("libutil-tests/data")
        .join(relative_path)
}

/// Test reading (deserializing) from upstream Nix JSON format.
pub fn test_upstream_json_from_json<T>(path: &Path, expected: &T)
where
    T: for<'de> Deserialize<'de> + PartialEq + Debug,
{
    let json_str = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {}", path.display(), e));

    let parsed: T = serde_json::from_str(&json_str)
        .unwrap_or_else(|e| panic!("Failed to parse {}: {}", path.display(), e));

    assert_eq!(parsed, *expected);
}

/// Test writing (serializing) to JSON and reading back (round-trip).
pub fn test_upstream_json_to_json<T>(path: &Path, value: &T)
where
    T: Serialize + for<'de> Deserialize<'de> + PartialEq + Debug,
{
    let json_str = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {}", path.display(), e));

    let json: serde_json::Value = serde_json::from_str(&json_str).unwrap();

    let serialized = serde_json::to_value(value).unwrap();
    assert_eq!(json, serialized);
}

/// Macro to generate both read and write tests for upstream JSON compatibility.
///
/// # Example
///
/// ```ignore
/// use harmonia_utils_test::test_upstream_json;
/// use harmonia_utils_test::json_upstream::libstore_test_data_path;
///
/// test_upstream_json!(
///     test_my_type,
///     libstore_test_data_path("my-type/example.json"),
///     MyType { field: "value".to_string() }
/// );
/// ```
#[macro_export]
macro_rules! test_upstream_json {
    ($test_name:ident, $path:expr, $value:expr) => {
        $crate::paste::paste! {
            #[test]
            fn [<$test_name _from_json>]() {
                $crate::json_upstream::test_upstream_json_from_json(&$path, &$value);
            }

            #[test]
            fn [<$test_name _to_json>]() {
                $crate::json_upstream::test_upstream_json_to_json(&$path, &$value);
            }
        }
    };
}
