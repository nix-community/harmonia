//! Realisation JSON tests

use crate::libstore_test_data_path;
use crate::test_upstream_json;
use harmonia_store_core::realisation::Realisation;

test_upstream_json!(
    test_realisation_simple,
    libstore_test_data_path("realisation/simple.json"),
    {
        Realisation {
            drv_path: "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-bar.drv".parse().unwrap(),
            output_name: "foo".parse().unwrap(),
            out_path: "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-foo".parse().unwrap(),
            signatures: Default::default(),
        }
    }
);

test_upstream_json!(
    test_realisation_with_signature,
    libstore_test_data_path("realisation/with-signature.json"),
    {
        Realisation {
            drv_path: "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-bar.drv".parse().unwrap(),
            output_name: "foo".parse().unwrap(),
            out_path: "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-foo.drv".parse().unwrap(),
            signatures: ["asdf:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=="]
                .into_iter()
                .map(|s| s.parse().unwrap())
                .collect(),
        }
    }
);

#[test]
fn test_realisation_with_signature_structured_from_json() {
    crate::test_upstream_json_from_json(
        &libstore_test_data_path("realisation/with-signature-structured.json"),
        &Realisation {
            drv_path: "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-bar.drv".parse().unwrap(),
            output_name: "foo".parse().unwrap(),
            out_path: "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-foo.drv".parse().unwrap(),
            signatures: ["asdf:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=="]
                .into_iter()
                .map(|s| s.parse().unwrap())
                .collect(),
        },
    );
}

#[test]
fn test_realisation_with_structured_signature_from_json() {
    crate::test_upstream_json_from_json(
        &libstore_test_data_path("realisation/with-structured-signature.json"),
        &Realisation {
            drv_path: "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-bar.drv".parse().unwrap(),
            output_name: "foo".parse().unwrap(),
            out_path: "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-foo".parse().unwrap(),
            signatures: ["asdf:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=="]
                .into_iter()
                .map(|s| s.parse().unwrap())
                .collect(),
        },
    );
}
