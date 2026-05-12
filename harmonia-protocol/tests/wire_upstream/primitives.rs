//! Worker protocol wire tests for primitive types.

use harmonia_protocol::daemon_wire::types2::BuildMode;
use harmonia_protocol::types::TrustLevel;

use crate::{no_features, test_upstream_wire};

test_upstream_wire!(string, "string", String, no_features());

test_upstream_wire!(build_mode, "build-mode", BuildMode, no_features());

test_upstream_wire!(
    optional_trusted_flag,
    "optional-trusted-flag",
    Option<TrustLevel>,
    no_features()
);
