//! ValidPathInfo JSON tests

use crate::libstore_test_data_path;
use harmonia_protocol::NarHash;
use harmonia_protocol::valid_path_info::UnkeyedValidPathInfo;
use harmonia_store_core::signature::Signature;
use harmonia_store_core::store_path::{ContentAddress, StoreDir};
use harmonia_utils_hash::Sha256;
use harmonia_utils_test::test_upstream_json;
use hex_literal::hex;
use std::collections::BTreeSet;

// The NAR hash used in upstream test data
// SRI: sha256-FePFYIlMuycIXPZbWi7LGEiMmZSX9FMbaQenWBzm1Sc=
const TEST_NAR_HASH: NarHash = NarHash::new(&hex!(
    "15e3c560894cbb27085cf65b5a2ecb18488c999497f4531b6907a7581ce6d527"
));

// The CA hash used in upstream test data
// SRI: sha256-EMIJ+giQ/gLIWoxmPKjno3zHZrxbGymgzGGyZvZBIdM=
const TEST_CA_HASH: Sha256 = Sha256::new(&hex!(
    "10c209fa0890fe02c85a8c663ca8e7a37cc766bc5b1b29a0cc61b266f64121d3"
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

fn pure_info() -> UnkeyedValidPathInfo {
    UnkeyedValidPathInfo {
        deriver: None,
        nar_hash: TEST_NAR_HASH,
        references: [
            "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-bar".parse().unwrap(),
            "n5wkd9frr45pa74if5gpz9j7mifg27fh-foo".parse().unwrap(),
        ]
        .into_iter()
        .collect(),
        registration_time: 0,
        nar_size: 34878,
        ultimate: false,
        signatures: BTreeSet::new(),
        ca: Some(ContentAddress::Recursive(TEST_CA_HASH.into())),
        store_dir: StoreDir::default(),
    }
}

fn impure_info() -> UnkeyedValidPathInfo {
    UnkeyedValidPathInfo {
        deriver: Some("g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-bar.drv".parse().unwrap()),
        nar_hash: TEST_NAR_HASH,
        references: [
            "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-bar".parse().unwrap(),
            "n5wkd9frr45pa74if5gpz9j7mifg27fh-foo".parse().unwrap(),
        ]
        .into_iter()
        .collect(),
        registration_time: 23423,
        nar_size: 34878,
        ultimate: true,
        signatures: [
            Signature::new("asdf".to_string()),
            Signature::new("qwer".to_string()),
        ]
        .into_iter()
        .collect(),
        ca: Some(ContentAddress::Recursive(TEST_CA_HASH.into())),
        store_dir: StoreDir::default(),
    }
}

// Test empty_pure.json - minimal valid path info (pure format, no impure fields)
test_upstream_json!(
    test_valid_path_info_empty_pure,
    libstore_test_data_path("path-info/json-2/empty_pure.json"),
    empty_info()
);

// Test empty_impure.json - empty but includes impure fields
test_upstream_json!(
    test_valid_path_info_empty_impure,
    libstore_test_data_path("path-info/json-2/empty_impure.json"),
    empty_info()
);

// Test pure.json - has ca, references, narSize but no impure fields
test_upstream_json!(
    test_valid_path_info_pure,
    libstore_test_data_path("path-info/json-2/pure.json"),
    pure_info()
);

// Test impure.json - full info with all fields
test_upstream_json!(
    test_valid_path_info_impure,
    libstore_test_data_path("path-info/json-2/impure.json"),
    impure_info()
);
