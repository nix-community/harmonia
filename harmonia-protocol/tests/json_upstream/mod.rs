//! Tests that verify JSON serialization matches upstream Nix format
//!
//! These tests use JSON test data from the upstream Nix repository.

pub use harmonia_utils_test::json_upstream::libstore_test_data_path;
pub use harmonia_utils_test::test_upstream_json;

mod build_result;
mod valid_path_info;
