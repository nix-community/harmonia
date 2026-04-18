//! Realisation JSON tests

use crate::libstore_test_data_path;
use crate::test_upstream_json;
use harmonia_store_core::realisation::{DrvOutput, Realisation, UnkeyedRealisation};
use harmonia_store_core::signature::Signature;

fn drv_output() -> DrvOutput {
    DrvOutput {
        drv_path: "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-bar.drv".parse().unwrap(),
        output_name: "foo".parse().unwrap(),
    }
}

test_upstream_json!(
    test_unkeyed_realisation_simple,
    libstore_test_data_path("realisation/unkeyed-simple.json"),
    {
        UnkeyedRealisation {
            out_path: "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-foo".parse().unwrap(),
            signatures: Default::default(),
        }
    }
);

test_upstream_json!(
    test_unkeyed_realisation_with_signature,
    libstore_test_data_path("realisation/unkeyed-with-signature.json"),
    {
        UnkeyedRealisation {
            out_path: "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-foo.drv".parse().unwrap(),
            signatures: ["asdf:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA==".parse::<Signature>().unwrap()].into(),
        }
    }
);

test_upstream_json!(
    test_realisation_simple,
    libstore_test_data_path("realisation/simple.json"),
    {
        Realisation {
            id: drv_output(),
            value: UnkeyedRealisation {
                out_path: "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-foo".parse().unwrap(),
                signatures: Default::default(),
            },
        }
    }
);

test_upstream_json!(
    test_realisation_with_signature,
    libstore_test_data_path("realisation/with-signature.json"),
    {
        Realisation {
            id: drv_output(),
            value: UnkeyedRealisation {
                out_path: "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-foo.drv".parse().unwrap(),
                signatures: ["asdf:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA==".parse::<Signature>().unwrap()].into(),
            },
        }
    }
);

#[test]
fn test_realisation_fingerprint_simple() {
    let r = Realisation {
        id: drv_output(),
        value: UnkeyedRealisation {
            out_path: "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-foo".parse().unwrap(),
            signatures: Default::default(),
        },
    };
    let expected = std::fs::read_to_string(libstore_test_data_path(
        "realisation/simple-fingerprint.txt",
    ))
    .unwrap();
    assert_eq!(r.fingerprint(), expected.trim_end());
}

#[test]
fn test_realisation_fingerprint_with_signature() {
    let r = Realisation {
        id: drv_output(),
        value: UnkeyedRealisation {
            out_path: "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-foo.drv".parse().unwrap(),
            signatures: ["asdf:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA==".parse::<Signature>().unwrap()].into(),
        },
    };
    let expected = std::fs::read_to_string(libstore_test_data_path(
        "realisation/with-signature-fingerprint.txt",
    ))
    .unwrap();
    assert_eq!(r.fingerprint(), expected.trim_end());
}
