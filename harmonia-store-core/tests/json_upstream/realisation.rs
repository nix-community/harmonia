//! Realisation JSON tests

use crate::libstore_test_data_path;
use crate::test_upstream_json;
use harmonia_store_core::realisation::{DrvOutput, Realisation, UnkeyedRealisation};
use harmonia_store_core::signature::Signature;

fn test_sig() -> Signature {
    Signature::from_parts("asdf", &[0u8; 64]).unwrap()
}

// ========== Realisation ==========

test_upstream_json!(
    test_realisation_simple,
    libstore_test_data_path("realisation/simple.json"),
    {
        Realisation {
            key: DrvOutput {
                drv_path: "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-bar.drv".parse().unwrap(),
                output_name: "foo".parse().unwrap(),
            },
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
            key: DrvOutput {
                drv_path: "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-bar.drv".parse().unwrap(),
                output_name: "foo".parse().unwrap(),
            },
            value: UnkeyedRealisation {
                out_path: "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-foo.drv".parse().unwrap(),
                signatures: [test_sig()].into(),
            },
        }
    }
);

// ========== UnkeyedRealisation ==========

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
            signatures: [test_sig()].into(),
        }
    }
);
