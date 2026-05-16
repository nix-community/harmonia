//! Worker protocol wire tests for `ContentAddress` and `Option<ContentAddress>`.

use harmonia_store_content_address::ContentAddress;

use crate::{no_features, test_upstream_wire};

test_upstream_wire!(
    content_address,
    "content-address",
    ContentAddress,
    no_features()
);

test_upstream_wire!(
    optional_content_address,
    "optional-content-address",
    Option<ContentAddress>,
    no_features()
);
