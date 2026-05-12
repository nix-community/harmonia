//! Worker protocol wire tests for `DrvOutput`.
//!
//! Only the `realisation-with-path-not-hash` feature variant is tested.
//! The legacy hash-string format (`sha256:<hex>!<name>`) is not
//! supported by our `DrvOutput` wire impl.

use harmonia_store_core::realisation::DrvOutput;

use crate::{realisation_with_path_features, test_upstream_wire};

test_upstream_wire!(
    drv_output_realisation_with_path,
    "drv-output-realisation-with-path-not-hash",
    DrvOutput,
    realisation_with_path_features()
);
