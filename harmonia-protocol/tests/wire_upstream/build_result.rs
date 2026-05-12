//! Worker protocol wire tests for `BuildResult`.
//!
//! Our minimum supported protocol version is
//! [`PROTOCOL_VERSION_MIN`](harmonia_protocol::version::PROTOCOL_VERSION_MIN)
//! (1.37), so we only test fixtures at that version and above:
//!
//! - **1.37**: All fields present. Legacy `builtOutputs` format with real
//!   `DrvOutput` hashes in keys. Read-only because our writer uses dummy
//!   hashes.
//! - **realisation-with-path-not-hash**: Protocol 1.38 with the feature
//!   flag. New compact `builtOutputs` format. Full round-trip.

use harmonia_store_build_result::BuildResult;

use crate::{
    no_features, realisation_with_path_features, test_upstream_wire, test_upstream_wire_read,
};

test_upstream_wire_read!(
    build_result_1_37,
    "build-result-1.37",
    BuildResult,
    no_features()
);

test_upstream_wire!(
    build_result_realisation_with_path,
    "build-result-realisation-with-path-not-hash",
    BuildResult,
    realisation_with_path_features()
);
