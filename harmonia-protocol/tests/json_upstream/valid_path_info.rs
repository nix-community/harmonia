//! ValidPathInfo JSON tests

use crate::libstore_test_data_path;
use harmonia_protocol::valid_path_info::UnkeyedValidPathInfo;
use harmonia_store_core::store_path::StoreDir;
use harmonia_utils_hash::NarHash;
use harmonia_utils_test::test_upstream_json;
use hex_literal::hex;
use std::collections::BTreeSet;

// The NAR hash used in upstream test data
// SRI: sha256-FePFYIlMuycIXPZbWi7LGEiMmZSX9FMbaQenWBzm1Sc=
const TEST_NAR_HASH: NarHash = NarHash::new(&hex!(
    "15e3c560894cbb27085cf65b5a2ecb18488c999497f4531b6907a7581ce6d527"
));

fn empty_info() -> UnkeyedValidPathInfo {
    UnkeyedValidPathInfo {
        deriver: None,
        nar_hash: TEST_NAR_HASH,
        references: BTreeSet::new(),
        registration_time: 0,
        nar_size: 0,
        ultimate: false,
        signatures: BTreeSet::new(),
        ca: None,
        store_dir: StoreDir::default(),
    }
}

// Test empty_pure.json - minimal valid path info (pure format, no impure fields)
test_upstream_json!(
    test_valid_path_info_empty_pure,
    libstore_test_data_path("path-info/json-2/empty_pure.json"),
    empty_info()
);
