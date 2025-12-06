//! ContentAddress JSON tests

use crate::libstore_test_data_path;
use crate::test_upstream_json;
use harmonia_store_core::store_path::ContentAddress;
use harmonia_utils_hash::{Algorithm, Hash};
use hex_literal::hex;

test_upstream_json!(
    test_content_address_text,
    libstore_test_data_path("content-address/text.json"),
    ContentAddress::Text(
        Hash::new(
            Algorithm::SHA256,
            &hex!("f0e4c2f76c58916ec258f246851bea091d14d4247a2fc3e18694461b1816e13b"),
        )
        .try_into()
        .unwrap()
    )
);

test_upstream_json!(
    test_content_address_nar,
    libstore_test_data_path("content-address/nar.json"),
    ContentAddress::Recursive(Hash::new(
        Algorithm::SHA256,
        &hex!("f6f2ea8f45d8a057c9566a33f99474da2e5c6a6604d736121650e2730c6fb0a3"),
    ))
);
