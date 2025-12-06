//! Hash JSON tests

use crate::test_upstream_json;
use crate::{libutil_test_data_path, test_upstream_json_from_json};
use harmonia_utils_hash::{Algorithm, Hash};
use hex_literal::hex;

// Base16/hex is the canonical form, so test both read and write
test_upstream_json!(
    test_hash_sha256_base16,
    libutil_test_data_path("hash/sha256-base16.json"),
    Hash::new(
        Algorithm::SHA256,
        &hex!("f0e4c2f76c58916ec258f246851bea091d14d4247a2fc3e18694461b1816e13b"),
    )
);

// Base64 is not the canonical form - read-only test
#[test]
fn test_hash_sha256_base64_from_json() {
    test_upstream_json_from_json(
        &libutil_test_data_path("hash/sha256-base64.json"),
        &Hash::new(
            Algorithm::SHA256,
            &hex!("f0e4c2f76c58916ec258f246851bea091d14d4247a2fc3e18694461b1816e13b"),
        ),
    );
}

// Nix32 is not the canonical form - read-only test
#[test]
fn test_hash_sha256_nix32_from_json() {
    test_upstream_json_from_json(
        &libutil_test_data_path("hash/sha256-nix32.json"),
        &Hash::new(
            Algorithm::SHA256,
            &hex!("f0e4c2f76c58916ec258f246851bea091d14d4247a2fc3e18694461b1816e13b"),
        ),
    );
}

// Simple test uses base16 - test both read and write
test_upstream_json!(
    test_hash_simple,
    libutil_test_data_path("hash/simple.json"),
    Hash::new(
        Algorithm::SHA256,
        &hex!("f0e4c2f76c58916ec258f246851bea091d14d4247a2fc3e18694461b1816e13b"),
    )
);
