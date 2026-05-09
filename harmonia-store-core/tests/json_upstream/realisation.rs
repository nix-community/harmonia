//! Realisation JSON tests

use crate::libstore_test_data_path;
use crate::read_upstream_json;
use crate::test_upstream_json;
use harmonia_store_core::realisation::{DrvOutput, Realisation, UnkeyedRealisation};
use harmonia_store_core::signature::{RawSignature, SIGNATURE_BYTES, SecretKey, Signature};

fn drv_output() -> DrvOutput {
    DrvOutput {
        drv_path: "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-bar.drv".parse().unwrap(),
        output_name: "foo".parse().unwrap(),
    }
}

fn test_sig() -> Signature {
    Signature {
        key_name: "asdf".to_string(),
        sig: RawSignature([0u8; SIGNATURE_BYTES]),
    }
}

/// Fixed test key from upstream Nix:
/// https://github.com/NixOS/nix/blob/4239a7ae2c7e79c567eacdbe2ab56195796acd91/src/libstore-tests/realisation.cc#L40
const TEST_SECRET_KEY: &str = "test-key:tU7tTvLcScf8pmz/eTV0BEtLmRsPpZfKaRcd0nCN+pysBZPHSeg61/u2oc7mIOewfuAY1V1BiX32homTaDJ2Jw==";

fn simple_realisation() -> Realisation {
    Realisation {
        key: drv_output(),
        value: UnkeyedRealisation {
            out_path: "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-foo".parse().unwrap(),
            signatures: Default::default(),
        },
    }
}

fn with_signature_realisation() -> Realisation {
    Realisation {
        key: drv_output(),
        value: UnkeyedRealisation {
            out_path: "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-foo.drv".parse().unwrap(),
            signatures: [test_sig()].into(),
        },
    }
}

// ========== Realisation ==========

test_upstream_json!(
    test_realisation_simple,
    libstore_test_data_path("realisation/simple.json"),
    { simple_realisation() }
);

test_upstream_json!(
    test_realisation_with_signature,
    libstore_test_data_path("realisation/with-signature.json"),
    { with_signature_realisation() }
);

#[test]
fn test_realisation_fingerprint_simple() {
    let r = simple_realisation();
    let expected = std::fs::read_to_string(libstore_test_data_path(
        "realisation/simple-fingerprint.txt",
    ))
    .unwrap();
    assert_eq!(r.value.fingerprint(&r.key), expected.trim_end());
}

#[test]
fn test_realisation_fingerprint_with_signature() {
    let r = with_signature_realisation();
    let expected = std::fs::read_to_string(libstore_test_data_path(
        "realisation/with-signature-fingerprint.txt",
    ))
    .unwrap();
    assert_eq!(r.value.fingerprint(&r.key), expected.trim_end());
}

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

// ========== Signing ==========

/// Signing produces the same signature as upstream Nix.
#[test]
fn sign_simple_matches_upstream() {
    let r = simple_realisation();
    let sk: SecretKey = TEST_SECRET_KEY.parse().unwrap();
    let sig = r.value.sign(&r.key, &sk);

    let expected: Signature =
        read_upstream_json(&libstore_test_data_path("realisation/simple-sig.json"));
    assert_eq!(sig, expected);
}

#[test]
fn sign_with_signature_matches_upstream() {
    let r = with_signature_realisation();
    let sk: SecretKey = TEST_SECRET_KEY.parse().unwrap();
    let sig = r.value.sign(&r.key, &sk);

    let expected: Signature = read_upstream_json(&libstore_test_data_path(
        "realisation/with-signature-sig.json",
    ));
    assert_eq!(sig, expected);
}

/// Verifying the upstream signatures works.
#[test]
fn verify_simple_upstream_signature() {
    let r = simple_realisation();
    let sk: SecretKey = TEST_SECRET_KEY.parse().unwrap();
    let pk = sk.to_public_key();

    let sig: Signature =
        read_upstream_json(&libstore_test_data_path("realisation/simple-sig.json"));
    assert!(pk.verify(r.value.fingerprint(&r.key).as_bytes(), &sig));
}

#[test]
fn verify_with_signature_upstream_signature() {
    let r = with_signature_realisation();
    let sk: SecretKey = TEST_SECRET_KEY.parse().unwrap();
    let pk = sk.to_public_key();

    let sig: Signature = read_upstream_json(&libstore_test_data_path(
        "realisation/with-signature-sig.json",
    ));
    assert!(pk.verify(r.value.fingerprint(&r.key).as_bytes(), &sig));
}
