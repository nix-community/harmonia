//! Tests that verify JSON serialization matches upstream Nix format
//!
//! These tests use JSON test data from the upstream Nix repository.

use harmonia_store_core::derived_path::{DerivedPath, OutputSpec, SingleDerivedPath};
use harmonia_store_core::realisation::Realisation;
use harmonia_store_core::store_path::StorePath;
use serde::{Deserialize, Serialize};
use std::fmt::Debug;
use std::path::PathBuf;

fn upstream_test_data_path() -> PathBuf {
    // NIX_UPSTREAM_SRC environment variable should be set by the flake
    let nix_src =
        std::env::var("NIX_UPSTREAM_SRC").expect("NIX_UPSTREAM_SRC environment variable not set");
    PathBuf::from(nix_src).join("src/libstore-tests/data")
}

fn test_upstream_json<T>(relative_path: &str, expected: T)
where
    T: Serialize + for<'de> Deserialize<'de> + PartialEq + Debug,
{
    let path = upstream_test_data_path().join(relative_path);
    let json = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {}", path.display(), e));

    let parsed: T = serde_json::from_str(&json)
        .unwrap_or_else(|e| panic!("Failed to parse {}: {}", path.display(), e));

    assert_eq!(parsed, expected);

    // Test round-trip
    let serialized = serde_json::to_value(&parsed).unwrap();
    let deserialized: T = serde_json::from_value(serialized).unwrap();
    assert_eq!(parsed, deserialized);
}

#[test]
fn test_store_path_simple() {
    test_upstream_json(
        "store-path/simple.json",
        "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-foo.drv"
            .parse::<StorePath>()
            .unwrap(),
    );
}

#[test]
fn test_output_spec_all() {
    test_upstream_json("outputs-spec/all.json", OutputSpec::All);
}

#[test]
fn test_output_spec_names() {
    test_upstream_json(
        "outputs-spec/names.json",
        OutputSpec::Named(["a", "b"].into_iter().map(|s| s.parse().unwrap()).collect()),
    );
}

#[test]
fn test_single_derived_path_opaque() {
    test_upstream_json(
        "derived-path/single_opaque.json",
        SingleDerivedPath::Opaque("g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-foo.drv".parse().unwrap()),
    );
}

#[test]
fn test_single_derived_path_built() {
    test_upstream_json(
        "derived-path/single_built.json",
        SingleDerivedPath::Built {
            drv_path: Box::new(SingleDerivedPath::Opaque(
                "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-foo.drv".parse().unwrap(),
            )),
            output: "bar".parse().unwrap(),
        },
    );
}

#[test]
fn test_single_derived_path_built_built() {
    test_upstream_json(
        "derived-path/single_built_built.json",
        SingleDerivedPath::Built {
            drv_path: Box::new(SingleDerivedPath::Built {
                drv_path: Box::new(SingleDerivedPath::Opaque(
                    "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-foo.drv".parse().unwrap(),
                )),
                output: "bar".parse().unwrap(),
            }),
            output: "baz".parse().unwrap(),
        },
    );
}

#[test]
fn test_derived_path_opaque() {
    test_upstream_json(
        "derived-path/multi_opaque.json",
        DerivedPath::Opaque("g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-foo.drv".parse().unwrap()),
    );
}

#[test]
fn test_realisation_simple() {
    use harmonia_store_core::realisation::DrvOutput;

    test_upstream_json(
        "realisation/simple.json",
        Realisation {
            id: "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad!foo"
                .parse::<DrvOutput>()
                .unwrap(),
            out_path: "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-foo.drv".parse().unwrap(),
            signatures: Default::default(),
            dependent_realisations: Default::default(),
        },
    );
}

#[test]
fn test_realisation_with_dependent() {
    use harmonia_store_core::realisation::DrvOutput;

    test_upstream_json(
        "realisation/with-dependent-realisations.json",
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
        },
    );
}
