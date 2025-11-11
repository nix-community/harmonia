//! Tests that verify JSON serialization matches upstream Nix format
//!
//! These tests use JSON test data from the upstream Nix repository.

use harmonia_store_core::derived_path::{DerivedPath, OutputSpec, SingleDerivedPath};
use harmonia_store_core::hash::{Algorithm, Hash};
use harmonia_store_core::realisation::Realisation;
use harmonia_store_core::store_path::StorePath;
use hex_literal::hex;
use serde::{Deserialize, Serialize};
use std::fmt::Debug;
use std::path::PathBuf;

fn upstream_test_data_path() -> PathBuf {
    // NIX_UPSTREAM_SRC environment variable should be set by the flake
    let nix_src =
        std::env::var("NIX_UPSTREAM_SRC").expect("NIX_UPSTREAM_SRC environment variable not set");
    PathBuf::from(nix_src).join("src")
}

fn libstore_test_data_path(relative_path: &str) -> PathBuf {
    upstream_test_data_path()
        .join("libstore-tests/data")
        .join(relative_path)
}

fn libutil_test_data_path(relative_path: &str) -> PathBuf {
    upstream_test_data_path()
        .join("libutil-tests/data")
        .join(relative_path)
}

/// Test reading (deserializing) from upstream Nix JSON format
fn test_upstream_json_read<T>(path: PathBuf, expected: T)
where
    T: for<'de> Deserialize<'de> + PartialEq + Debug,
{
    let json = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {}", path.display(), e));

    let parsed: T = serde_json::from_str(&json)
        .unwrap_or_else(|e| panic!("Failed to parse {}: {}", path.display(), e));

    assert_eq!(parsed, expected);
}

/// Test writing (serializing) to JSON and reading back (round-trip)
fn test_upstream_json_write<T>(value: T)
where
    T: Serialize + for<'de> Deserialize<'de> + PartialEq + Debug,
{
    let serialized = serde_json::to_value(&value).unwrap();
    let deserialized: T = serde_json::from_value(serialized).unwrap();
    assert_eq!(value, deserialized);
}

/// Macro to generate both read and write tests for upstream JSON compatibility
macro_rules! test_upstream_json {
    ($test_name:ident, $path:expr, $value:expr) => {
        paste::paste! {
            #[test]
            fn [<$test_name _read>]() {
                test_upstream_json_read($path, $value);
            }

            #[test]
            fn [<$test_name _write>]() {
                test_upstream_json_write($value);
            }
        }
    };
}

test_upstream_json!(
    test_store_path_simple,
    libstore_test_data_path("store-path/simple.json"),
    "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-foo.drv"
        .parse::<StorePath>()
        .unwrap()
);

test_upstream_json!(
    test_output_spec_all,
    libstore_test_data_path("outputs-spec/all.json"),
    OutputSpec::All
);

test_upstream_json!(
    test_output_spec_names,
    libstore_test_data_path("outputs-spec/names.json"),
    OutputSpec::Named(["a", "b"].into_iter().map(|s| s.parse().unwrap()).collect())
);

test_upstream_json!(
    test_single_derived_path_opaque,
    libstore_test_data_path("derived-path/single_opaque.json"),
    SingleDerivedPath::Opaque("g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-foo.drv".parse().unwrap())
);

test_upstream_json!(
    test_single_derived_path_built,
    libstore_test_data_path("derived-path/single_built.json"),
    SingleDerivedPath::Built {
        drv_path: Box::new(SingleDerivedPath::Opaque(
            "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-foo.drv".parse().unwrap(),
        )),
        output: "bar".parse().unwrap(),
    }
);

test_upstream_json!(
    test_single_derived_path_built_built,
    libstore_test_data_path("derived-path/single_built_built.json"),
    SingleDerivedPath::Built {
        drv_path: Box::new(SingleDerivedPath::Built {
            drv_path: Box::new(SingleDerivedPath::Opaque(
                "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-foo.drv".parse().unwrap(),
            )),
            output: "bar".parse().unwrap(),
        }),
        output: "baz".parse().unwrap(),
    }
);

test_upstream_json!(
    test_derived_path_opaque,
    libstore_test_data_path("derived-path/multi_opaque.json"),
    DerivedPath::Opaque("g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-foo.drv".parse().unwrap())
);

test_upstream_json!(
    test_derived_path_built,
    libstore_test_data_path("derived-path/mutli_built.json"),
    DerivedPath::Built {
        drv_path: SingleDerivedPath::Opaque(
            "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-foo.drv".parse().unwrap(),
        ),
        outputs: OutputSpec::Named(
            ["bar", "baz"]
                .into_iter()
                .map(|s| s.parse().unwrap())
                .collect(),
        ),
    }
);

test_upstream_json!(
    test_derived_path_built_built,
    libstore_test_data_path("derived-path/multi_built_built.json"),
    DerivedPath::Built {
        drv_path: SingleDerivedPath::Built {
            drv_path: Box::new(SingleDerivedPath::Opaque(
                "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-foo.drv".parse().unwrap(),
            )),
            output: "bar".parse().unwrap(),
        },
        outputs: OutputSpec::Named(
            ["baz", "quux"]
                .into_iter()
                .map(|s| s.parse().unwrap())
                .collect(),
        ),
    }
);

test_upstream_json!(
    test_derived_path_built_built_wildcard,
    libstore_test_data_path("derived-path/multi_built_built_wildcard.json"),
    DerivedPath::Built {
        drv_path: SingleDerivedPath::Built {
            drv_path: Box::new(SingleDerivedPath::Opaque(
                "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-foo.drv".parse().unwrap(),
            )),
            output: "bar".parse().unwrap(),
        },
        outputs: OutputSpec::All,
    }
);

test_upstream_json!(
    test_realisation_simple,
    libstore_test_data_path("realisation/simple.json"),
    {
        use harmonia_store_core::realisation::DrvOutput;
        Realisation {
            id: "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad!foo"
                .parse::<DrvOutput>()
                .unwrap(),
            out_path: "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-foo.drv".parse().unwrap(),
            signatures: Default::default(),
            dependent_realisations: Default::default(),
        }
    }
);

test_upstream_json!(
    test_realisation_with_dependent,
    libstore_test_data_path("realisation/with-dependent-realisations.json"),
    {
        use harmonia_store_core::realisation::DrvOutput;
        Realisation {
            id: "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad!foo"
                .parse::<DrvOutput>()
                .unwrap(),
            out_path: "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-foo.drv".parse().unwrap(),
            signatures: Default::default(),
            dependent_realisations: [(
                "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad!foo"
                    .parse::<DrvOutput>()
                    .unwrap(),
                "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-foo.drv".parse().unwrap(),
            )]
            .into_iter()
            .collect(),
        }
    }
);

test_upstream_json!(
    test_hash_sha256_base64,
    libutil_test_data_path("hash/sha256-base64.json"),
    Hash::new(
        Algorithm::SHA256,
        &hex!("f0e4c2f76c58916ec258f246851bea091d14d4247a2fc3e18694461b1816e13b"),
    )
);

test_upstream_json!(
    test_hash_sha256_base16,
    libutil_test_data_path("hash/sha256-base16.json"),
    Hash::new(
        Algorithm::SHA256,
        &hex!("f0e4c2f76c58916ec258f246851bea091d14d4247a2fc3e18694461b1816e13b"),
    )
);

test_upstream_json!(
    test_hash_sha256_nix32,
    libutil_test_data_path("hash/sha256-nix32.json"),
    Hash::new(
        Algorithm::SHA256,
        &hex!("f0e4c2f76c58916ec258f246851bea091d14d4247a2fc3e18694461b1816e13b"),
    )
);

test_upstream_json!(
    test_hash_simple,
    libutil_test_data_path("hash/simple.json"),
    Hash::new(
        Algorithm::SHA256,
        &hex!("f0e4c2f76c58916ec258f246851bea091d14d4247a2fc3e18694461b1816e13b"),
    )
);
