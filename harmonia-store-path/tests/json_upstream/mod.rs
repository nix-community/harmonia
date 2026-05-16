//! Tests that verify JSON serialization matches upstream Nix format
//!
//! These tests use JSON test data from the upstream Nix repository.

pub use harmonia_utils_test::json_upstream::{
    libstore_test_data_path, libutil_test_data_path, read_upstream_json,
    test_upstream_json_from_json,
};
pub use harmonia_utils_test::test_upstream_json;

mod store_path;
