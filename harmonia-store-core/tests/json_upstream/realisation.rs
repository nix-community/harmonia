//! Realisation JSON tests

use crate::libstore_test_data_path;
use crate::test_upstream_json;
use harmonia_store_core::realisation::{DrvOutput, Realisation};

test_upstream_json!(
    test_realisation_simple,
    libstore_test_data_path("realisation/simple.json"),
    {
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
