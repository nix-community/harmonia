//! Worker protocol wire tests for `StorePath` and `Option<StorePath>`.

use harmonia_store_core::store_path::StorePath;

use crate::{no_features, test_upstream_wire};

test_upstream_wire!(store_path, "store-path", StorePath, no_features());

test_upstream_wire!(
    optional_store_path,
    "optional-store-path",
    Option<StorePath>,
    no_features()
);
