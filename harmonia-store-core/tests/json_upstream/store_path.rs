//! StorePath JSON tests

use crate::libstore_test_data_path;
use crate::test_upstream_json;
use harmonia_store_core::store_path::StorePath;

test_upstream_json!(
    test_store_path_simple,
    libstore_test_data_path("store-path/simple.json"),
    "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-foo.drv"
        .parse::<StorePath>()
        .unwrap()
);
