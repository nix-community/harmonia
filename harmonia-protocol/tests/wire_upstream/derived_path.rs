//! Worker protocol wire tests for `DerivedPath`.
//!
//! The wire format has been stable since protocol 1.30, so we test the
//! `derived-path-1.30` fixture even though 1.30 is below our minimum
//! version number — the encoding is identical at 1.37+.

use harmonia_store_derivation::derived_path::DerivedPath;

use crate::{no_features, test_upstream_wire};

test_upstream_wire!(
    derived_path_1_30,
    "derived-path-1.30",
    DerivedPath,
    no_features()
);
