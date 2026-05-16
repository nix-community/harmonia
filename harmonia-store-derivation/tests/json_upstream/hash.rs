//! Hash JSON tests

use crate::libutil_test_data_path;
use crate::test_upstream_json;
use harmonia_utils_hash::{Algorithm, Hash};
use hex_literal::hex;

// SHA256 test - upstream uses SRI format
test_upstream_json!(
    test_hash_sha256,
    libutil_test_data_path("hash/sha256.json"),
    Hash::new(
        Algorithm::SHA256,
        &hex!("f0e4c2f76c58916ec258f246851bea091d14d4247a2fc3e18694461b1816e13b"),
    )
);
