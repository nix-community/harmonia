//! Tests that verify worker protocol wire serialization matches upstream Nix
//! binary fixtures.
//!
//! The upstream JSON files are the source of truth for expected values.
//! The `.bin` files are the expected wire encoding. Each fixture is a
//! tuple of values that are concatenated in the binary stream (no length
//! prefix).

use harmonia_protocol::version::{FEATURE_REALISATION_WITH_PATH, FeatureSet};

mod test_infra;
use test_infra::{test_upstream_wire, test_upstream_wire_read, test_upstream_wire_single};

mod build_result;
mod collections;
mod content_address;
mod derived_path;
mod drv_output;
mod primitives;
mod realisation;
mod store_path;
mod valid_path_info;

fn no_features() -> FeatureSet {
    Default::default()
}

fn realisation_with_path_features() -> FeatureSet {
    [FEATURE_REALISATION_WITH_PATH.to_owned()].into()
}
