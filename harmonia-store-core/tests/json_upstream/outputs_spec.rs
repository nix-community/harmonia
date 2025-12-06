//! OutputSpec JSON tests

use crate::libstore_test_data_path;
use crate::test_upstream_json;
use harmonia_store_core::derived_path::OutputSpec;

test_upstream_json!(
    test_output_spec_all,
    libstore_test_data_path("outputs-spec/all.json"),
    OutputSpec::All
);

test_upstream_json!(
    test_output_spec_names,
    libstore_test_data_path("outputs-spec/names.json"),
    OutputSpec::Named(["a", "b"].into_iter().map(|s| s.parse().unwrap()).collect())
);
