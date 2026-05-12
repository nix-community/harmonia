//! Worker protocol wire tests for `UnkeyedRealisation` and `Realisation`.
//!
//! Only the `realisation-with-path-not-hash` feature variants are tested.
//! The legacy format uses hash-based `DrvOutput` strings which our wire
//! impls do not support.

use harmonia_store_core::realisation::{Realisation, UnkeyedRealisation};

use crate::{realisation_with_path_features, test_upstream_wire_single};

test_upstream_wire_single!(
    unkeyed_realisation_realisation_with_path,
    "unkeyed-realisation-realisation-with-path-not-hash",
    UnkeyedRealisation,
    realisation_with_path_features()
);

test_upstream_wire_single!(
    realisation_realisation_with_path,
    "realisation-realisation-with-path-not-hash",
    Realisation,
    realisation_with_path_features()
);
