//! Derivation JSON tests

use std::sync::Arc;

use crate::libstore_test_data_path;
use crate::test_upstream_json;
use harmonia_store_core::derivation::{
    BasicDerivation, Derivation, DerivationOutput, StructuredAttrs,
};
use harmonia_store_core::derived_path::{OutputName, SingleDerivedPath};
use harmonia_store_core::placeholder::Placeholder;
use harmonia_store_core::store_path::ContentAddressMethodAlgorithm;
use harmonia_utils_hash::Algorithm;
use std::collections::{BTreeMap, BTreeSet};

fn ca_floating_output() -> DerivationOutput {
    DerivationOutput::CAFloating(ContentAddressMethodAlgorithm::Recursive(Algorithm::SHA256))
}

test_upstream_json!(
    test_derivation_simple,
    libstore_test_data_path("derivation/simple-derivation.json"),
    Derivation {
        name: "simple-derivation".parse().unwrap(),
        outputs: BTreeMap::new(),
        inputs: {
            let dep2_drv = Arc::new(SingleDerivedPath::Opaque(
                "c015dhfh5l0lp6wxyvdn7bmwhbbr6hr9-dep2.drv".parse().unwrap(),
            ));

            [
                // Source path
                SingleDerivedPath::Opaque("c015dhfh5l0lp6wxyvdn7bmwhbbr6hr9-dep1".parse().unwrap()),
                // Built derivation outputs
                SingleDerivedPath::Built {
                    drv_path: dep2_drv.clone(),
                    output: "cat".parse().unwrap(),
                },
                SingleDerivedPath::Built {
                    drv_path: dep2_drv,
                    output: "dog".parse().unwrap(),
                },
            ]
            .into_iter()
            .collect::<BTreeSet<_>>()
        },
        platform: bytes::Bytes::from("wasm-sel4"),
        builder: bytes::Bytes::from("foo"),
        args: vec![bytes::Bytes::from("bar"), bytes::Bytes::from("baz")],
        env: {
            let mut map = BTreeMap::new();
            map.insert(bytes::Bytes::from("BIG_BAD"), bytes::Bytes::from("WOLF"));
            map
        },
        structured_attrs: None,
    }
);

test_upstream_json!(
    test_derivation_ca_structured_attrs,
    libstore_test_data_path("derivation/ca/advanced-attributes-structured-attrs.json"),
    Derivation {
        name: "advanced-attributes-structured-attrs".parse().unwrap(),
        outputs: {
            let mut map = BTreeMap::new();
            map.insert("out".parse().unwrap(), ca_floating_output());
            map.insert("bin".parse().unwrap(), ca_floating_output());
            map.insert("dev".parse().unwrap(), ca_floating_output());
            map
        },
        inputs: {
            let foo_drv = Arc::new(SingleDerivedPath::Opaque(
                "j56sf12rxpcv5swr14vsjn5cwm6bj03h-foo.drv".parse().unwrap(),
            ));
            let bar_drv = Arc::new(SingleDerivedPath::Opaque(
                "qnml92yh97a6fbrs2m5qg5cqlc8vni58-bar.drv".parse().unwrap(),
            ));

            [
                SingleDerivedPath::Opaque("qnml92yh97a6fbrs2m5qg5cqlc8vni58-bar.drv".parse().unwrap()),
                SingleDerivedPath::Built {
                    drv_path: foo_drv.clone(),
                    output: "dev".parse().unwrap(),
                },
                SingleDerivedPath::Built {
                    drv_path: foo_drv,
                    output: "out".parse().unwrap(),
                },
                SingleDerivedPath::Built {
                    drv_path: bar_drv.clone(),
                    output: "dev".parse().unwrap(),
                },
                SingleDerivedPath::Built {
                    drv_path: bar_drv,
                    output: "out".parse().unwrap(),
                },
            ]
            .into_iter()
            .collect::<BTreeSet<_>>()
        },
        platform: bytes::Bytes::from("my-system"),
        builder: bytes::Bytes::from("/bin/bash"),
        args: vec![bytes::Bytes::from("-c"), bytes::Bytes::from("echo hello > $out")],
        env: {
            let mut map = BTreeMap::new();
            let out: OutputName = "out".parse().unwrap();
            let bin: OutputName = "bin".parse().unwrap();
            let dev: OutputName = "dev".parse().unwrap();
            let out_placeholder = Placeholder::standard_output(&out).render();
            let bin_placeholder = Placeholder::standard_output(&bin).render();
            let dev_placeholder = Placeholder::standard_output(&dev).render();
            map.insert(
                bytes::Bytes::from("out"),
                bytes::Bytes::from(out_placeholder.to_string_lossy().to_string()),
            );
            map.insert(
                bytes::Bytes::from("bin"),
                bytes::Bytes::from(bin_placeholder.to_string_lossy().to_string()),
            );
            map.insert(
                bytes::Bytes::from("dev"),
                bytes::Bytes::from(dev_placeholder.to_string_lossy().to_string()),
            );
            map
        },
        structured_attrs: Some(StructuredAttrs {
            attrs: serde_json::from_value(serde_json::json!({
                "__darwinAllowLocalNetworking": true,
                "__impureHostDeps": ["/usr/bin/ditto"],
                "__noChroot": true,
                "__sandboxProfile": "sandcastle",
                "allowSubstitutes": false,
                "builder": "/bin/bash",
                "exportReferencesGraph": {
                    "refs1": ["/164j69y6zir9z0339n8pjigg3rckinlr77bxsavzizdaaljb7nh9"],
                    "refs2": ["/nix/store/qnml92yh97a6fbrs2m5qg5cqlc8vni58-bar.drv"]
                },
                "impureEnvVars": ["UNICORN"],
                "name": "advanced-attributes-structured-attrs",
                "outputChecks": {
                    "bin": {
                        "disallowedReferences": ["/0nyw57wm2iicnm9rglvjmbci3ikmcp823czdqdzdcgsnnwqps71g", "dev"],
                        "disallowedRequisites": ["/07f301yqyz8c6wf6bbbavb2q39j4n8kmcly1s09xadyhgy6x2wr8"]
                    },
                    "dev": {
                        "maxClosureSize": 5909,
                        "maxSize": 789
                    },
                    "out": {
                        "allowedReferences": ["/164j69y6zir9z0339n8pjigg3rckinlr77bxsavzizdaaljb7nh9"],
                        "allowedRequisites": ["/0nr45p69vn6izw9446wsh9bng9nndhvn19kpsm4n96a5mycw0s4z", "bin"]
                    }
                },
                "outputHashAlgo": "sha256",
                "outputHashMode": "recursive",
                "outputs": ["out", "bin", "dev"],
                "preferLocalBuild": true,
                "requiredSystemFeatures": ["rainbow", "uid-range"],
                "system": "my-system"
            })).unwrap(),
        }),
    }
);

test_upstream_json!(
    test_derivation_ca_structured_attrs_defaults,
    libstore_test_data_path("derivation/ca/advanced-attributes-structured-attrs-defaults.json"),
    Derivation {
        name: "advanced-attributes-structured-attrs-defaults"
            .parse()
            .unwrap(),
        outputs: {
            let mut map = BTreeMap::new();
            map.insert("out".parse().unwrap(), ca_floating_output());
            map.insert("dev".parse().unwrap(), ca_floating_output());
            map
        },
        inputs: BTreeSet::new(),
        platform: bytes::Bytes::from("my-system"),
        builder: bytes::Bytes::from("/bin/bash"),
        args: vec![
            bytes::Bytes::from("-c"),
            bytes::Bytes::from("echo hello > $out"),
        ],
        env: {
            let mut map = BTreeMap::new();
            let out: OutputName = "out".parse().unwrap();
            let dev: OutputName = "dev".parse().unwrap();
            let out_placeholder = Placeholder::standard_output(&out).render();
            let dev_placeholder = Placeholder::standard_output(&dev).render();
            map.insert(
                bytes::Bytes::from("out"),
                bytes::Bytes::from(out_placeholder.to_string_lossy().to_string()),
            );
            map.insert(
                bytes::Bytes::from("dev"),
                bytes::Bytes::from(dev_placeholder.to_string_lossy().to_string()),
            );
            map
        },
        structured_attrs: Some(StructuredAttrs {
            attrs: serde_json::from_value(serde_json::json!({
                "builder": "/bin/bash",
                "name": "advanced-attributes-structured-attrs-defaults",
                "outputHashAlgo": "sha256",
                "outputHashMode": "recursive",
                "outputs": ["out", "dev"],
                "system": "my-system"
            }))
            .unwrap(),
        }),
    }
);

test_upstream_json!(
    test_derivation_ia_structured_attrs,
    libstore_test_data_path("derivation/ia/advanced-attributes-structured-attrs.json"),
    Derivation {
        name: "advanced-attributes-structured-attrs".parse().unwrap(),
        outputs: {
            let mut map = BTreeMap::new();
            map.insert(
                "out".parse().unwrap(),
                DerivationOutput::InputAddressed("h1vh648d3p088kdimy0r8ngpfx7c3nzw-advanced-attributes-structured-attrs".parse().unwrap()),
            );
            map.insert(
                "bin".parse().unwrap(),
                DerivationOutput::InputAddressed("cnpasdljgkhnwaf78cf3qygcp4qbki1c-advanced-attributes-structured-attrs-bin".parse().unwrap()),
            );
            map.insert(
                "dev".parse().unwrap(),
                DerivationOutput::InputAddressed("ijq6mwpa9jbnpnl33qldfqihrr38kprx-advanced-attributes-structured-attrs-dev".parse().unwrap()),
            );
            map
        },
        inputs: {
            let foo_drv = Arc::new(SingleDerivedPath::Opaque(
                "afc3vbjbzql750v2lp8gxgaxsajphzih-foo.drv".parse().unwrap(),
            ));
            let bar_drv = Arc::new(SingleDerivedPath::Opaque(
                "vj2i49jm2868j2fmqvxm70vlzmzvgv14-bar.drv".parse().unwrap(),
            ));

            [
                SingleDerivedPath::Opaque("vj2i49jm2868j2fmqvxm70vlzmzvgv14-bar.drv".parse().unwrap()),
                SingleDerivedPath::Built {
                    drv_path: foo_drv.clone(),
                    output: "dev".parse().unwrap(),
                },
                SingleDerivedPath::Built {
                    drv_path: foo_drv,
                    output: "out".parse().unwrap(),
                },
                SingleDerivedPath::Built {
                    drv_path: bar_drv.clone(),
                    output: "dev".parse().unwrap(),
                },
                SingleDerivedPath::Built {
                    drv_path: bar_drv,
                    output: "out".parse().unwrap(),
                },
            ]
            .into_iter()
            .collect::<BTreeSet<_>>()
        },
        platform: bytes::Bytes::from("my-system"),
        builder: bytes::Bytes::from("/bin/bash"),
        args: vec![bytes::Bytes::from("-c"), bytes::Bytes::from("echo hello > $out")],
        env: {
            let mut map = BTreeMap::new();
            map.insert(bytes::Bytes::from("out"), bytes::Bytes::from("/nix/store/h1vh648d3p088kdimy0r8ngpfx7c3nzw-advanced-attributes-structured-attrs"));
            map.insert(bytes::Bytes::from("bin"), bytes::Bytes::from("/nix/store/cnpasdljgkhnwaf78cf3qygcp4qbki1c-advanced-attributes-structured-attrs-bin"));
            map.insert(bytes::Bytes::from("dev"), bytes::Bytes::from("/nix/store/ijq6mwpa9jbnpnl33qldfqihrr38kprx-advanced-attributes-structured-attrs-dev"));
            map
        },
        structured_attrs: Some(StructuredAttrs {
            attrs: serde_json::from_value(serde_json::json!({
                "__darwinAllowLocalNetworking": true,
                "__impureHostDeps": ["/usr/bin/ditto"],
                "__noChroot": true,
                "__sandboxProfile": "sandcastle",
                "allowSubstitutes": false,
                "builder": "/bin/bash",
                "exportReferencesGraph": {
                    "refs1": ["/nix/store/p0hax2lzvjpfc2gwkk62xdglz0fcqfzn-foo"],
                    "refs2": ["/nix/store/vj2i49jm2868j2fmqvxm70vlzmzvgv14-bar.drv"]
                },
                "impureEnvVars": ["UNICORN"],
                "name": "advanced-attributes-structured-attrs",
                "outputChecks": {
                    "bin": {
                        "disallowedReferences": ["/nix/store/r5cff30838majxk5mp3ip2diffi8vpaj-bar", "dev"],
                        "disallowedRequisites": ["/nix/store/9b61w26b4avv870dw0ymb6rw4r1hzpws-bar-dev"]
                    },
                    "dev": {
                        "maxClosureSize": 5909,
                        "maxSize": 789
                    },
                    "out": {
                        "allowedReferences": ["/nix/store/p0hax2lzvjpfc2gwkk62xdglz0fcqfzn-foo"],
                        "allowedRequisites": ["/nix/store/z0rjzy29v9k5qa4nqpykrbzirj7sd43v-foo-dev", "bin"]
                    }
                },
                "outputs": ["out", "bin", "dev"],
                "preferLocalBuild": true,
                "requiredSystemFeatures": ["rainbow", "uid-range"],
                "system": "my-system"
            })).unwrap(),
        }),
    }
);

test_upstream_json!(
    test_derivation_ia_structured_attrs_defaults,
    libstore_test_data_path("derivation/ia/advanced-attributes-structured-attrs-defaults.json"),
    Derivation {
        name: "advanced-attributes-structured-attrs-defaults"
            .parse()
            .unwrap(),
        outputs: {
            let mut map = BTreeMap::new();
            map.insert(
                "out".parse().unwrap(),
                DerivationOutput::InputAddressed("f8f8nvnx32bxvyxyx2ff7akbvwhwd9dw-advanced-attributes-structured-attrs-defaults".parse().unwrap()),
            );
            map.insert(
                "dev".parse().unwrap(),
                DerivationOutput::InputAddressed("8bazivnbipbyi569623skw5zm91z6kc2-advanced-attributes-structured-attrs-defaults-dev".parse().unwrap()),
            );
            map
        },
        inputs: BTreeSet::new(),
        platform: bytes::Bytes::from("my-system"),
        builder: bytes::Bytes::from("/bin/bash"),
        args: vec![
            bytes::Bytes::from("-c"),
            bytes::Bytes::from("echo hello > $out"),
        ],
        env: {
            let mut map = BTreeMap::new();
            map.insert(bytes::Bytes::from("out"), bytes::Bytes::from("/nix/store/f8f8nvnx32bxvyxyx2ff7akbvwhwd9dw-advanced-attributes-structured-attrs-defaults"));
            map.insert(bytes::Bytes::from("dev"), bytes::Bytes::from("/nix/store/8bazivnbipbyi569623skw5zm91z6kc2-advanced-attributes-structured-attrs-defaults-dev"));
            map
        },
        structured_attrs: Some(StructuredAttrs {
            attrs: serde_json::from_value(serde_json::json!({
                "builder": "/bin/bash",
                "name": "advanced-attributes-structured-attrs-defaults",
                "outputs": ["out", "dev"],
                "system": "my-system"
            }))
            .unwrap(),
        }),
    }
);

// try-resolve "before" tests (Derivation JSON format)

test_upstream_json!(
    test_try_resolve_no_inputs_before,
    libstore_test_data_path("derivation/try-resolve/no-inputs-before.json"),
    Derivation {
        name: "no-inputs".parse().unwrap(),
        outputs: {
            let mut map = BTreeMap::new();
            map.insert("out".parse().unwrap(), ca_floating_output());
            map
        },
        inputs: BTreeSet::new(),
        platform: bytes::Bytes::from("x86_64-linux"),
        builder: bytes::Bytes::from("/bin/bash"),
        args: vec![],
        env: {
            let mut map = BTreeMap::new();
            map.insert(bytes::Bytes::from("FOO"), bytes::Bytes::from("bar"));
            map
        },
        structured_attrs: None,
    }
);

test_upstream_json!(
    test_try_resolve_with_inputs_before,
    libstore_test_data_path("derivation/try-resolve/with-inputs-before.json"),
    {
        let dep1_drv = Arc::new(SingleDerivedPath::Opaque(
            "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-dep1.drv".parse().unwrap(),
        ));
        let dep2_drv = Arc::new(SingleDerivedPath::Opaque(
            "h1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-dep2.drv".parse().unwrap(),
        ));

        let dep1_drv_path: harmonia_store_core::store_path::StorePath =
            "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-dep1.drv".parse().unwrap();
        let dep2_drv_path: harmonia_store_core::store_path::StorePath =
            "h1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-dep2.drv".parse().unwrap();

        let placeholder1_out =
            Placeholder::ca_output(&dep1_drv_path, &"out".parse().unwrap()).render();
        let placeholder1_dev =
            Placeholder::ca_output(&dep1_drv_path, &"dev".parse().unwrap()).render();
        let placeholder2_out =
            Placeholder::ca_output(&dep2_drv_path, &"out".parse().unwrap()).render();

        let ph1_out = placeholder1_out.to_string_lossy().to_string();
        let ph1_dev = placeholder1_dev.to_string_lossy().to_string();
        let ph2_out = placeholder2_out.to_string_lossy().to_string();

        Derivation {
            name: "with-inputs".parse().unwrap(),
            outputs: {
                let mut map = BTreeMap::new();
                map.insert("dev".parse().unwrap(), ca_floating_output());
                map.insert("out".parse().unwrap(), ca_floating_output());
                map
            },
            inputs: [
                SingleDerivedPath::Built {
                    drv_path: dep1_drv.clone(),
                    output: "dev".parse().unwrap(),
                },
                SingleDerivedPath::Built {
                    drv_path: dep1_drv,
                    output: "out".parse().unwrap(),
                },
                SingleDerivedPath::Built {
                    drv_path: dep2_drv,
                    output: "out".parse().unwrap(),
                },
            ]
            .into_iter()
            .collect::<BTreeSet<_>>(),
            platform: bytes::Bytes::from("x86_64-linux"),
            builder: bytes::Bytes::from("/bin/bash"),
            args: vec![],
            env: {
                let mut map = BTreeMap::new();
                map.insert(bytes::Bytes::from("DEP1_DEV"), bytes::Bytes::from(ph1_dev));
                map.insert(
                    bytes::Bytes::from("DEP1_OUT"),
                    bytes::Bytes::from(format!("prefix-{ph1_out}-suffix")),
                );
                map.insert(
                    bytes::Bytes::from("DEP2"),
                    bytes::Bytes::from(ph2_out.clone()),
                );
                map
            },
            structured_attrs: Some(StructuredAttrs {
                attrs: serde_json::from_value(serde_json::json!({
                    "dep1out": ph1_out,
                    "nested": {
                        "dep2": format!("before {ph2_out} after")
                    }
                }))
                .unwrap(),
            }),
        }
    }
);

test_upstream_json!(
    test_try_resolve_resolution_failure_before,
    libstore_test_data_path("derivation/try-resolve/resolution-failure-before.json"),
    {
        let dep_drv = Arc::new(SingleDerivedPath::Opaque(
            "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-dep.drv".parse().unwrap(),
        ));

        Derivation {
            name: "resolution-failure".parse().unwrap(),
            outputs: {
                let mut map = BTreeMap::new();
                map.insert("out".parse().unwrap(), ca_floating_output());
                map
            },
            inputs: [SingleDerivedPath::Built {
                drv_path: dep_drv,
                output: "out".parse().unwrap(),
            }]
            .into_iter()
            .collect::<BTreeSet<_>>(),
            platform: bytes::Bytes::from("x86_64-linux"),
            builder: bytes::Bytes::from("/bin/bash"),
            args: vec![],
            env: BTreeMap::new(),
            structured_attrs: None,
        }
    }
);

// try-resolve "after" tests (BasicDerivation / resolved derivation JSON format)

test_upstream_json!(
    test_try_resolve_no_inputs_after,
    libstore_test_data_path("derivation/try-resolve/no-inputs-after.json"),
    BasicDerivation {
        name: "no-inputs".parse().unwrap(),
        outputs: {
            let mut map = BTreeMap::new();
            map.insert("out".parse().unwrap(), ca_floating_output());
            map
        },
        inputs: BTreeSet::new(),
        platform: bytes::Bytes::from("x86_64-linux"),
        builder: bytes::Bytes::from("/bin/bash"),
        args: vec![],
        env: {
            let mut map = BTreeMap::new();
            map.insert(bytes::Bytes::from("FOO"), bytes::Bytes::from("bar"));
            map
        },
        structured_attrs: None,
    }
);

test_upstream_json!(
    test_try_resolve_with_inputs_after,
    libstore_test_data_path("derivation/try-resolve/with-inputs-after.json"),
    BasicDerivation {
        name: "with-inputs".parse().unwrap(),
        outputs: {
            let mut map = BTreeMap::new();
            map.insert("dev".parse().unwrap(), ca_floating_output());
            map.insert("out".parse().unwrap(), ca_floating_output());
            map
        },
        inputs: [
            "f1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-dep1-out".parse().unwrap(),
            "i1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-dep2-out".parse().unwrap(),
            "j1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-dep1-dev".parse().unwrap(),
        ]
        .into_iter()
        .collect(),
        platform: bytes::Bytes::from("x86_64-linux"),
        builder: bytes::Bytes::from("/bin/bash"),
        args: vec![],
        env: {
            let mut map = BTreeMap::new();
            map.insert(
                bytes::Bytes::from("DEP1_DEV"),
                bytes::Bytes::from("/nix/store/j1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-dep1-dev"),
            );
            map.insert(
                bytes::Bytes::from("DEP1_OUT"),
                bytes::Bytes::from(
                    "prefix-/nix/store/f1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-dep1-out-suffix",
                ),
            );
            map.insert(
                bytes::Bytes::from("DEP2"),
                bytes::Bytes::from("/nix/store/i1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-dep2-out"),
            );
            map
        },
        structured_attrs: Some(StructuredAttrs {
            attrs: serde_json::from_value(serde_json::json!({
                "dep1out": "/nix/store/f1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-dep1-out",
                "nested": {
                    "dep2": "before /nix/store/i1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-dep2-out after"
                }
            }))
            .unwrap(),
        }),
    }
);
