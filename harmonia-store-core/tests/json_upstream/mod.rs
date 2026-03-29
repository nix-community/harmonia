//! Tests that verify JSON serialization matches upstream Nix format
//!
//! These tests use JSON test data from the upstream Nix repository.

pub use harmonia_utils_test::json_upstream::{
    libstore_test_data_path, libutil_test_data_path, test_upstream_json_from_json,
};
pub use harmonia_utils_test::test_upstream_json;

// Submodules organized by type
mod content_address;
mod derivation;
mod derivation_output;
mod derived_path;
mod hash;
mod outputs_spec;
mod realisation;
mod store_path;
