//! DerivationOptions JSON tests
//!
//! Tests for derivation build-time options (sandboxing, reference checking, etc.)

use std::sync::Arc;

use crate::libstore_test_data_path;
use crate::test_upstream_json;
use harmonia_store_core::derived_path::SingleDerivedPath;

// IA (input-addressed) DerivationOptions tests
test_upstream_json!(
    test_derivation_options_ia_defaults,
    libstore_test_data_path("derivation/ia/defaults.json"),
    {
        use harmonia_store_core::derivation::BasicDerivationOptions;
        BasicDerivationOptions::default()
    }
);

test_upstream_json!(
    test_derivation_options_ia_all_set,
    libstore_test_data_path("derivation/ia/all_set.json"),
    {
        use harmonia_store_core::derivation::{
            BasicDerivationOptions, OutputCheckSpec, OutputChecks,
        };
        use harmonia_store_core::drv_ref::DrvRef;
        use harmonia_store_core::store_path::StorePath;
        use std::collections::{BTreeMap, BTreeSet};

        BasicDerivationOptions {
            output_checks: OutputChecks::ForAllOutputs(OutputCheckSpec {
                ignore_self_refs: true,
                max_size: None,
                max_closure_size: None,
                allowed_references: Some(
                    ["p0hax2lzvjpfc2gwkk62xdglz0fcqfzn-foo"
                        .parse::<StorePath>()
                        .unwrap()]
                    .into_iter()
                    .map(DrvRef::External)
                    .collect(),
                ),
                allowed_requisites: Some(
                    [
                        DrvRef::SelfOutput("bin".parse().unwrap()),
                        DrvRef::External(
                            "z0rjzy29v9k5qa4nqpykrbzirj7sd43v-foo-dev"
                                .parse::<StorePath>()
                                .unwrap(),
                        ),
                    ]
                    .into_iter()
                    .collect(),
                ),
                disallowed_references: [
                    DrvRef::SelfOutput("dev".parse().unwrap()),
                    DrvRef::External(
                        "r5cff30838majxk5mp3ip2diffi8vpaj-bar"
                            .parse::<StorePath>()
                            .unwrap(),
                    ),
                ]
                .into_iter()
                .collect(),
                disallowed_requisites: ["9b61w26b4avv870dw0ymb6rw4r1hzpws-bar-dev"
                    .parse::<StorePath>()
                    .unwrap()]
                .into_iter()
                .map(DrvRef::External)
                .collect(),
            }),
            unsafe_discard_references: BTreeMap::new(),
            pass_as_file: BTreeSet::new(),
            export_references_graph: [
                (
                    "refs1".to_string(),
                    ["p0hax2lzvjpfc2gwkk62xdglz0fcqfzn-foo"
                        .parse::<StorePath>()
                        .unwrap()]
                    .into_iter()
                    .collect(),
                ),
                (
                    "refs2".to_string(),
                    ["vj2i49jm2868j2fmqvxm70vlzmzvgv14-bar.drv"
                        .parse::<StorePath>()
                        .unwrap()]
                    .into_iter()
                    .collect(),
                ),
            ]
            .into_iter()
            .collect(),
            additional_sandbox_profile: "sandcastle".to_string(),
            no_chroot: true,
            impure_host_deps: ["/usr/bin/ditto".to_string()].into_iter().collect(),
            impure_env_vars: ["UNICORN".to_string()].into_iter().collect(),
            allow_local_networking: true,
            required_system_features: ["rainbow".to_string(), "uid-range".to_string()]
                .into_iter()
                .collect(),
            prefer_local_build: true,
            allow_substitutes: false,
        }
    }
);

test_upstream_json!(
    test_derivation_options_ia_structured_attrs_defaults,
    libstore_test_data_path("derivation/ia/structuredAttrs_defaults.json"),
    {
        use harmonia_store_core::derivation::{BasicDerivationOptions, OutputChecks};
        use std::collections::BTreeMap;

        BasicDerivationOptions {
            output_checks: OutputChecks::PerOutput(BTreeMap::new()),
            ..Default::default()
        }
    }
);

test_upstream_json!(
    test_derivation_options_ia_structured_attrs_all_set,
    libstore_test_data_path("derivation/ia/structuredAttrs_all_set.json"),
    {
        use harmonia_store_core::derivation::{
            BasicDerivationOptions, OutputCheckSpec, OutputChecks,
        };
        use harmonia_store_core::drv_ref::DrvRef;
        use harmonia_store_core::store_path::StorePath;
        use std::collections::{BTreeMap, BTreeSet};

        BasicDerivationOptions {
            output_checks: OutputChecks::PerOutput({
                let mut map = BTreeMap::new();
                map.insert(
                    "bin".parse().unwrap(),
                    OutputCheckSpec {
                        ignore_self_refs: false,
                        max_size: None,
                        max_closure_size: None,
                        allowed_references: None,
                        allowed_requisites: None,
                        disallowed_references: [
                            DrvRef::SelfOutput("dev".parse().unwrap()),
                            DrvRef::External(
                                "r5cff30838majxk5mp3ip2diffi8vpaj-bar"
                                    .parse::<StorePath>()
                                    .unwrap(),
                            ),
                        ]
                        .into_iter()
                        .collect(),
                        disallowed_requisites: ["9b61w26b4avv870dw0ymb6rw4r1hzpws-bar-dev"
                            .parse::<StorePath>()
                            .unwrap()]
                        .into_iter()
                        .map(DrvRef::External)
                        .collect(),
                    },
                );
                map.insert(
                    "dev".parse().unwrap(),
                    OutputCheckSpec {
                        ignore_self_refs: false,
                        max_size: Some(789),
                        max_closure_size: Some(5909),
                        allowed_references: None,
                        allowed_requisites: None,
                        disallowed_references: BTreeSet::new(),
                        disallowed_requisites: BTreeSet::new(),
                    },
                );
                map.insert(
                    "out".parse().unwrap(),
                    OutputCheckSpec {
                        ignore_self_refs: false,
                        max_size: None,
                        max_closure_size: None,
                        allowed_references: Some(
                            ["p0hax2lzvjpfc2gwkk62xdglz0fcqfzn-foo"
                                .parse::<StorePath>()
                                .unwrap()]
                            .into_iter()
                            .map(DrvRef::External)
                            .collect(),
                        ),
                        allowed_requisites: Some(
                            [
                                DrvRef::SelfOutput("bin".parse().unwrap()),
                                DrvRef::External(
                                    "z0rjzy29v9k5qa4nqpykrbzirj7sd43v-foo-dev"
                                        .parse::<StorePath>()
                                        .unwrap(),
                                ),
                            ]
                            .into_iter()
                            .collect(),
                        ),
                        disallowed_references: BTreeSet::new(),
                        disallowed_requisites: BTreeSet::new(),
                    },
                );
                map
            }),
            unsafe_discard_references: BTreeMap::new(),
            pass_as_file: BTreeSet::new(),
            export_references_graph: [
                (
                    "refs1".to_string(),
                    ["p0hax2lzvjpfc2gwkk62xdglz0fcqfzn-foo"
                        .parse::<StorePath>()
                        .unwrap()]
                    .into_iter()
                    .collect(),
                ),
                (
                    "refs2".to_string(),
                    ["vj2i49jm2868j2fmqvxm70vlzmzvgv14-bar.drv"
                        .parse::<StorePath>()
                        .unwrap()]
                    .into_iter()
                    .collect(),
                ),
            ]
            .into_iter()
            .collect(),
            additional_sandbox_profile: "sandcastle".to_string(),
            no_chroot: true,
            impure_host_deps: ["/usr/bin/ditto".to_string()].into_iter().collect(),
            impure_env_vars: ["UNICORN".to_string()].into_iter().collect(),
            allow_local_networking: true,
            required_system_features: ["rainbow".to_string(), "uid-range".to_string()]
                .into_iter()
                .collect(),
            prefer_local_build: true,
            allow_substitutes: false,
        }
    }
);

// CA (content-addressed) DerivationOptions tests (use SingleDerivedPath for inputs)
test_upstream_json!(
    test_derivation_options_ca_all_set,
    libstore_test_data_path("derivation/ca/all_set.json"),
    {
        use harmonia_store_core::derivation::{
            FullDerivationOptions, OutputCheckSpec, OutputChecks,
        };
        use harmonia_store_core::drv_ref::DrvRef;
        use std::collections::{BTreeMap, BTreeSet};

        let foo_drv = Arc::new(SingleDerivedPath::Opaque(
            "j56sf12rxpcv5swr14vsjn5cwm6bj03h-foo.drv".parse().unwrap(),
        ));
        let bar_drv = Arc::new(SingleDerivedPath::Opaque(
            "qnml92yh97a6fbrs2m5qg5cqlc8vni58-bar.drv".parse().unwrap(),
        ));

        FullDerivationOptions {
            output_checks: OutputChecks::ForAllOutputs(OutputCheckSpec {
                ignore_self_refs: true,
                max_size: None,
                max_closure_size: None,
                allowed_references: Some(
                    [SingleDerivedPath::Built {
                        drv_path: foo_drv.clone(),
                        output: "out".parse().unwrap(),
                    }]
                    .into_iter()
                    .map(DrvRef::External)
                    .collect(),
                ),
                allowed_requisites: Some(
                    [
                        DrvRef::SelfOutput("bin".parse().unwrap()),
                        DrvRef::External(SingleDerivedPath::Built {
                            drv_path: foo_drv.clone(),
                            output: "dev".parse().unwrap(),
                        }),
                    ]
                    .into_iter()
                    .collect(),
                ),
                disallowed_references: [
                    DrvRef::SelfOutput("dev".parse().unwrap()),
                    DrvRef::External(SingleDerivedPath::Built {
                        drv_path: bar_drv.clone(),
                        output: "out".parse().unwrap(),
                    }),
                ]
                .into_iter()
                .collect(),
                disallowed_requisites: [SingleDerivedPath::Built {
                    drv_path: bar_drv.clone(),
                    output: "dev".parse().unwrap(),
                }]
                .into_iter()
                .map(DrvRef::External)
                .collect(),
            }),
            unsafe_discard_references: BTreeMap::new(),
            pass_as_file: BTreeSet::new(),
            export_references_graph: [
                (
                    "refs1".to_string(),
                    [SingleDerivedPath::Built {
                        drv_path: foo_drv.clone(),
                        output: "out".parse().unwrap(),
                    }]
                    .into_iter()
                    .collect(),
                ),
                (
                    "refs2".to_string(),
                    [SingleDerivedPath::Opaque(
                        "qnml92yh97a6fbrs2m5qg5cqlc8vni58-bar.drv".parse().unwrap(),
                    )]
                    .into_iter()
                    .collect(),
                ),
            ]
            .into_iter()
            .collect(),
            additional_sandbox_profile: "sandcastle".to_string(),
            no_chroot: true,
            impure_host_deps: ["/usr/bin/ditto".to_string()].into_iter().collect(),
            impure_env_vars: ["UNICORN".to_string()].into_iter().collect(),
            allow_local_networking: true,
            required_system_features: ["rainbow".to_string(), "uid-range".to_string()]
                .into_iter()
                .collect(),
            prefer_local_build: true,
            allow_substitutes: false,
        }
    }
);

test_upstream_json!(
    test_derivation_options_ca_structured_attrs_all_set,
    libstore_test_data_path("derivation/ca/structuredAttrs_all_set.json"),
    {
        use harmonia_store_core::derivation::{
            FullDerivationOptions, OutputCheckSpec, OutputChecks,
        };
        use harmonia_store_core::drv_ref::DrvRef;
        use std::collections::{BTreeMap, BTreeSet};

        let foo_drv = Arc::new(SingleDerivedPath::Opaque(
            "j56sf12rxpcv5swr14vsjn5cwm6bj03h-foo.drv".parse().unwrap(),
        ));
        let bar_drv = Arc::new(SingleDerivedPath::Opaque(
            "qnml92yh97a6fbrs2m5qg5cqlc8vni58-bar.drv".parse().unwrap(),
        ));

        FullDerivationOptions {
            output_checks: OutputChecks::PerOutput({
                let mut map = BTreeMap::new();
                map.insert(
                    "bin".parse().unwrap(),
                    OutputCheckSpec {
                        ignore_self_refs: false,
                        max_size: None,
                        max_closure_size: None,
                        allowed_references: None,
                        allowed_requisites: None,
                        disallowed_references: [
                            DrvRef::SelfOutput("dev".parse().unwrap()),
                            DrvRef::External(SingleDerivedPath::Built {
                                drv_path: bar_drv.clone(),
                                output: "out".parse().unwrap(),
                            }),
                        ]
                        .into_iter()
                        .collect(),
                        disallowed_requisites: [SingleDerivedPath::Built {
                            drv_path: bar_drv.clone(),
                            output: "dev".parse().unwrap(),
                        }]
                        .into_iter()
                        .map(DrvRef::External)
                        .collect(),
                    },
                );
                map.insert(
                    "dev".parse().unwrap(),
                    OutputCheckSpec {
                        ignore_self_refs: false,
                        max_size: Some(789),
                        max_closure_size: Some(5909),
                        allowed_references: None,
                        allowed_requisites: None,
                        disallowed_references: BTreeSet::new(),
                        disallowed_requisites: BTreeSet::new(),
                    },
                );
                map.insert(
                    "out".parse().unwrap(),
                    OutputCheckSpec {
                        ignore_self_refs: false,
                        max_size: None,
                        max_closure_size: None,
                        allowed_references: Some(
                            [SingleDerivedPath::Built {
                                drv_path: foo_drv.clone(),
                                output: "out".parse().unwrap(),
                            }]
                            .into_iter()
                            .map(DrvRef::External)
                            .collect(),
                        ),
                        allowed_requisites: Some(
                            [
                                DrvRef::SelfOutput("bin".parse().unwrap()),
                                DrvRef::External(SingleDerivedPath::Built {
                                    drv_path: foo_drv.clone(),
                                    output: "dev".parse().unwrap(),
                                }),
                            ]
                            .into_iter()
                            .collect(),
                        ),
                        disallowed_references: BTreeSet::new(),
                        disallowed_requisites: BTreeSet::new(),
                    },
                );
                map
            }),
            unsafe_discard_references: BTreeMap::new(),
            pass_as_file: BTreeSet::new(),
            export_references_graph: [
                (
                    "refs1".to_string(),
                    [SingleDerivedPath::Built {
                        drv_path: foo_drv.clone(),
                        output: "out".parse().unwrap(),
                    }]
                    .into_iter()
                    .collect(),
                ),
                (
                    "refs2".to_string(),
                    [SingleDerivedPath::Opaque(
                        "qnml92yh97a6fbrs2m5qg5cqlc8vni58-bar.drv".parse().unwrap(),
                    )]
                    .into_iter()
                    .collect(),
                ),
            ]
            .into_iter()
            .collect(),
            additional_sandbox_profile: "sandcastle".to_string(),
            no_chroot: true,
            impure_host_deps: ["/usr/bin/ditto".to_string()].into_iter().collect(),
            impure_env_vars: ["UNICORN".to_string()].into_iter().collect(),
            allow_local_networking: true,
            required_system_features: ["rainbow".to_string(), "uid-range".to_string()]
                .into_iter()
                .collect(),
            prefer_local_build: true,
            allow_substitutes: false,
        }
    }
);
