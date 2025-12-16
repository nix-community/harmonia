//! Tests that verify JSON serialization matches upstream Nix format
//!
//! These tests use JSON test data from the upstream Nix repository.

pub use harmonia_utils_test::json_upstream::{
    libstore_test_data_path, test_upstream_json_from_json, test_upstream_json_to_json,
};

mod valid_path_info;
