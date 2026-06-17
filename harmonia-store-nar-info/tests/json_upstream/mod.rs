//! Tests that verify JSON serialization matches upstream Nix format

use harmonia_store_content_address::ContentAddress;
use harmonia_store_nar_info::{NarInfo, UnkeyedNarInfo, format_narinfo_txt, parse_narinfo_txt};
use harmonia_store_path::{StoreDir, StorePath};
use harmonia_store_path_info::{NarHash, Pure, UnkeyedValidPathInfo};
use harmonia_utils_signature::Signature;
use harmonia_utils_test::{
    json_upstream::{libstore_test_data_path, read_upstream_json},
    test_upstream_json,
};
use hex_literal::hex;
use std::num::NonZero;

// The NAR hash used in upstream test data
// SRI: sha256-FePFYIlMuycIXPZbWi7LGEiMmZSX9FMbaQenWBzm1Sc=
const TEST_NAR_HASH: NarHash = NarHash::new(&hex!(
    "15e3c560894cbb27085cf65b5a2ecb18488c999497f4531b6907a7581ce6d527"
));

// The CA hash used in upstream test data
// SRI: sha256-EMIJ+giQ/gLIWoxmPKjno3zHZrxbGymgzGGyZvZBIdM=
const TEST_CA_HASH: harmonia_utils_hash::Sha256 = harmonia_utils_hash::Sha256::new(&hex!(
    "10c209fa0890fe02c85a8c663ca8e7a37cc766bc5b1b29a0cc61b266f64121d3"
));

fn pure_info() -> UnkeyedValidPathInfo {
    impure_info().info.into_pure()
}

fn impure_info() -> UnkeyedNarInfo {
    UnkeyedNarInfo {
        info: UnkeyedValidPathInfo {
            deriver: Some("g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-bar.drv".parse().unwrap()),
            nar_hash: TEST_NAR_HASH,
            references: [
                "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-bar".parse().unwrap(),
                "n5wkd9frr45pa74if5gpz9j7mifg27fh-foo".parse().unwrap(),
            ]
            .into_iter()
            .collect(),
            registration_time: NonZero::new(23423),
            nar_size: 34878,
            ultimate: true,
            signatures: [
                "asdf:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA==".parse::<Signature>().unwrap(),
                "qwer:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA==".parse::<Signature>().unwrap(),
            ]
            .into_iter()
            .collect(),
            ca: Some(ContentAddress::NixArchive(TEST_CA_HASH.into())),
            store_dir: StoreDir::default(),
        },
        url: Some("nar/1w1fff338fvdw53sqgamddn1b2xgds473pv6y13gizdbqjv4i5p3.nar.xz".into()),
        compression: Some("xz".into()),
        download_hash: Some(harmonia_utils_hash::Hash::new(
            harmonia_utils_hash::Algorithm::SHA256,
            &hex!("15e3c560894cbb27085cf65b5a2ecb18488c999497f4531b6907a7581ce6d527"),
        )),
        download_size: Some(4029176),
    }
}

// Pure format (same as path-info pure format)
test_upstream_json!(
    test_nar_info_pure,
    libstore_test_data_path("nar-info/json-3/pure.json"),
    Pure(pure_info())
);

// Impure format
test_upstream_json!(
    test_nar_info_impure,
    libstore_test_data_path("nar-info/json-3/impure.json"),
    impure_info()
);

/// Re-rendering is the stable check because the text format drops `registrationTime` and `ultimate`.
fn assert_text_round_trip(info: UnkeyedNarInfo) {
    let store_dir = StoreDir::default();
    let path: StorePath = "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-foo".parse().unwrap();
    let narinfo = NarInfo { path, info };

    let text = format_narinfo_txt(&store_dir, &narinfo);
    let parsed = parse_narinfo_txt(&store_dir, std::str::from_utf8(&text).unwrap()).unwrap();

    assert_eq!(parsed.info.info.references, narinfo.info.info.references);
    assert_eq!(parsed.info.info.signatures, narinfo.info.info.signatures);
    assert_eq!(parsed.info.info.ca, narinfo.info.info.ca);
    assert_eq!(format_narinfo_txt(&store_dir, &parsed), text);
}

#[test]
fn nar_info_text_round_trip() {
    assert_text_round_trip(impure_info());
}

#[test]
fn nar_info_text_round_trip_upstream() {
    let info = read_upstream_json(&libstore_test_data_path("nar-info/json-3/impure.json"));
    assert_text_round_trip(info);
}
