//! DerivedPath and SingleDerivedPath JSON tests

use std::sync::Arc;

use crate::libstore_test_data_path;
use crate::test_upstream_json;
use harmonia_store_core::derived_path::{DerivedPath, OutputSpec, SingleDerivedPath};

test_upstream_json!(
    test_single_derived_path_opaque,
    libstore_test_data_path("derived-path/single_opaque.json"),
    SingleDerivedPath::Opaque("g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-foo.drv".parse().unwrap())
);

test_upstream_json!(
    test_single_derived_path_built,
    libstore_test_data_path("derived-path/single_built.json"),
    SingleDerivedPath::Built {
        drv_path: Arc::new(SingleDerivedPath::Opaque(
            "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-foo.drv".parse().unwrap(),
        )),
        output: "bar".parse().unwrap(),
    }
);

test_upstream_json!(
    test_single_derived_path_built_built,
    libstore_test_data_path("derived-path/single_built_built.json"),
    SingleDerivedPath::Built {
        drv_path: Arc::new(SingleDerivedPath::Built {
            drv_path: Arc::new(SingleDerivedPath::Opaque(
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
        drv_path: Arc::new(SingleDerivedPath::Opaque(
            "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-foo.drv".parse().unwrap(),
        )),
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
        drv_path: Arc::new(SingleDerivedPath::Built {
            drv_path: Arc::new(SingleDerivedPath::Opaque(
                "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-foo.drv".parse().unwrap(),
            )),
            output: "bar".parse().unwrap(),
        }),
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
        drv_path: Arc::new(SingleDerivedPath::Built {
            drv_path: Arc::new(SingleDerivedPath::Opaque(
                "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-foo.drv".parse().unwrap(),
            )),
            output: "bar".parse().unwrap(),
        }),
        outputs: OutputSpec::All,
    }
);
