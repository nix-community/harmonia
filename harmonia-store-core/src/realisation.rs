use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;

use derive_more::Display;

use serde::{Deserialize, Serialize};
use serde_with::{DeserializeFromStr, SerializeDisplay};
use thiserror::Error;

use crate::derived_path::OutputName;
use crate::hash::fmt::Any;
use crate::hash::{self, Hash};
use crate::signature::Signature;
use crate::store_path::{StorePath, StorePathNameError};

#[derive(
    Debug,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Clone,
    Display,
    SerializeDisplay,
    DeserializeFromStr,
)]
#[display("{drv_hash:x}!{output_name}")]
pub struct DrvOutput {
    pub drv_hash: hash::Hash,
    pub output_name: OutputName,
}

#[derive(Debug, PartialEq, Clone, Error)]
pub enum ParseDrvOutputError {
    #[error("derivation output {0}")]
    Hash(
        #[from]
        #[source]
        hash::fmt::ParseHashError,
    ),
    #[error("derivation output has {0}")]
    OutputName(
        #[from]
        #[source]
        StorePathNameError,
    ),
    #[error("missing '!' in derivation output '{0}'")]
    InvalidDerivationOutputId(String),
}

impl FromStr for DrvOutput {
    type Err = ParseDrvOutputError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some((drv_hash_s, output_name_s)) = s.split_once('!') {
            let drv_hash = drv_hash_s.parse::<Any<Hash>>()?.into_hash();
            let output_name = output_name_s.parse()?;
            Ok(DrvOutput {
                drv_hash,
                output_name,
            })
        } else {
            Err(ParseDrvOutputError::InvalidDerivationOutputId(s.into()))
        }
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Realisation {
    pub id: DrvOutput,
    #[serde(
        serialize_with = "serialize_store_path_base",
        deserialize_with = "deserialize_store_path_base"
    )]
    pub out_path: StorePath,
    pub signatures: BTreeSet<Signature>,
    #[serde(default, with = "store_path_map_as_base")]
    pub dependent_realisations: BTreeMap<DrvOutput, StorePath>,
}

fn serialize_store_path_base<S>(path: &StorePath, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(&path.to_base_path())
}

fn deserialize_store_path_base<'de, D>(deserializer: D) -> Result<StorePath, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    StorePath::from_base_path(&s).map_err(serde::de::Error::custom)
}

mod store_path_map_as_base {
    use super::*;
    use std::collections::BTreeMap;

    pub fn serialize<S>(
        map: &BTreeMap<DrvOutput, StorePath>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap;
        let mut s = serializer.serialize_map(Some(map.len()))?;
        for (k, v) in map {
            s.serialize_entry(&k.to_string(), &v.to_base_path())?;
        }
        s.end()
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<BTreeMap<DrvOutput, StorePath>, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let map: BTreeMap<String, String> = BTreeMap::deserialize(deserializer)?;
        let mut result = BTreeMap::new();
        for (k, v) in map {
            let key = k.parse().map_err(serde::de::Error::custom)?;
            let value = StorePath::from_base_path(&v).map_err(serde::de::Error::custom)?;
            result.insert(key, value);
        }
        Ok(result)
    }
}

pub type DrvOutputs = BTreeMap<DrvOutput, Realisation>;

#[cfg(any(test, feature = "test"))]
pub mod arbitrary {
    use crate::signature::proptests::arb_signatures;

    use super::*;
    use ::proptest::prelude::*;
    use ::proptest::sample::SizeRange;

    impl Arbitrary for DrvOutput {
        type Parameters = ();
        type Strategy = BoxedStrategy<DrvOutput>;

        fn arbitrary_with(_args: Self::Parameters) -> Self::Strategy {
            arb_drv_output().boxed()
        }
    }

    prop_compose! {
        pub fn arb_drv_output()
        (
            drv_hash in any::<hash::Hash>(),
            output_name in any::<OutputName>(),
        ) -> DrvOutput
        {
            DrvOutput { drv_hash, output_name }
        }
    }

    pub fn arb_drv_outputs(size: impl Into<SizeRange>) -> impl Strategy<Value = DrvOutputs> {
        let size = size.into();
        let min_size = size.start();
        prop::collection::vec(arb_realisation(), size)
            .prop_map(|r| {
                let mut ret = BTreeMap::new();
                for value in r {
                    ret.insert(value.id.clone(), value);
                }
                ret
            })
            .prop_filter("BTreeMap minimum size", move |m| m.len() >= min_size)
    }

    impl Arbitrary for Realisation {
        type Parameters = ();
        type Strategy = BoxedStrategy<Realisation>;

        fn arbitrary_with(_args: Self::Parameters) -> Self::Strategy {
            arb_realisation().boxed()
        }
    }

    prop_compose! {
        pub fn arb_realisation()
        (
            id in any::<DrvOutput>(),
            out_path in any::<StorePath>(),
            signatures in arb_signatures(),
            dependent_realisations in  prop::collection::btree_map(
                arb_drv_output(),
                any::<StorePath>(),
                0..50),
        ) -> Realisation
        {
            Realisation {
                id, out_path, signatures, dependent_realisations,
            }
        }
    }
}

#[cfg(test)]
mod unittests {
    use rstest::rstest;

    use crate::btree_map;
    use crate::derived_path::OutputName;
    use crate::hash::Hash;
    use crate::hash::fmt::Any;
    use crate::set;

    use super::{DrvOutput, Realisation};

    #[rstest]
    #[case("sha256:248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1!out", DrvOutput {
        drv_hash: "sha256:248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1".parse::<Any<Hash>>().unwrap().into_hash(),
        output_name: OutputName::default(),
    })]
    #[case("sha256:1h86vccx9vgcyrkj3zv4b7j3r8rrc0z0r4r6q3jvhf06s9hnm394!out_put", DrvOutput {
        drv_hash: "sha256:1h86vccx9vgcyrkj3zv4b7j3r8rrc0z0r4r6q3jvhf06s9hnm394".parse::<Any<Hash>>().unwrap().into_hash(),
        output_name: "out_put".parse().unwrap(),
    })]
    fn parse_drv_output(#[case] value: &str, #[case] expected: DrvOutput) {
        let actual: DrvOutput = value.parse().unwrap();
        assert_eq!(actual, expected);
    }

    #[rstest]
    #[should_panic = "missing '!' in derivation output 'sha256:1h86vccx9vgcyrkj3zv4b7j3r8rrc0z0r4r6q3jvhf06s9hnm394'"]
    #[case("sha256:1h86vccx9vgcyrkj3zv4b7j3r8rrc0z0r4r6q3jvhf06s9hnm394")]
    #[should_panic = "derivation output hash 'sha256:1h86vccx9vgcyrkj3zv4b7j3r8rrc0z0r4r6q3jvhf06s9hnm39' has wrong length for hash type 'sha256'"]
    #[case("sha256:1h86vccx9vgcyrkj3zv4b7j3r8rrc0z0r4r6q3jvhf06s9hnm39!out")]
    #[should_panic = "derivation output has invalid name symbol '{' at position 3"]
    #[case("sha256:1h86vccx9vgcyrkj3zv4b7j3r8rrc0z0r4r6q3jvhf06s9hnm394!out{put")]
    fn parse_drv_output_failure(#[case] value: &str) {
        let actual = value.parse::<DrvOutput>().unwrap_err();
        panic!("{actual}");
    }

    #[rstest]
    #[case(DrvOutput {
        drv_hash: "sha256:248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1".parse::<Any<Hash>>().unwrap().into_hash(),
        output_name: OutputName::default(),
    }, "sha256:248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1!out")]
    #[case(DrvOutput {
        drv_hash: "sha256:1h86vccx9vgcyrkj3zv4b7j3r8rrc0z0r4r6q3jvhf06s9hnm394".parse::<Any<Hash>>().unwrap().into_hash(),
        output_name: OutputName::default(),
    }, "sha256:248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1!out")]
    #[case(DrvOutput {
        drv_hash: "sha1:y5q4drg5558zk8aamsx6xliv3i23x644".parse::<Any<Hash>>().unwrap().into_hash(),
        output_name: "out_put".parse().unwrap(),
    }, "sha1:84983e441c3bd26ebaae4aa1f95129e5e54670f1!out_put")]
    fn display_drv_output(#[case] value: DrvOutput, #[case] expected: &str) {
        assert_eq!(value.to_string(), expected);
    }

    #[rstest]
    #[case(
        "{\"dependentRealisations\":{\"sha256:ba7816bf8f01cfea414140de5dae2223b00361a496177a9cf410ff61f20015ad!dev\":\"7h7qgvs4kgzsn8a6rb273saxyqh4jxlz-konsole-18.12.3-dev\",\"sha256:ba7816bf8f01cfea414140de5dae2223b00361a696177a9cf410ff61f20015ad!bin\":\"7h7qgvs4kgzsn8a6rb273saxyqh4jxlz-konsole-18.12.3-bin\"},\"id\":\"sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad!out\",\"outPath\":\"7h7qgvs4kgzsn8a6rb273saxyqh4jxlz-konsole-18.12.3\",\"signatures\":[\"cache.nixos.org-1:0CpHca+06TwFp9VkMyz5OaphT3E8mnS+1SWymYlvFaghKSYPCMQ66TS1XPAr1+y9rfQZPLaHrBjjnIRktE/nAA==\"]}",
        Realisation {
            id: "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad!out".parse().unwrap(),
            out_path: "7h7qgvs4kgzsn8a6rb273saxyqh4jxlz-konsole-18.12.3".parse().unwrap(),
            signatures: set!["cache.nixos.org-1:0CpHca+06TwFp9VkMyz5OaphT3E8mnS+1SWymYlvFaghKSYPCMQ66TS1XPAr1+y9rfQZPLaHrBjjnIRktE/nAA=="],
            dependent_realisations: btree_map![
                "sha256:ba7816bf8f01cfea414140de5dae2223b00361a496177a9cf410ff61f20015ad!dev" => "7h7qgvs4kgzsn8a6rb273saxyqh4jxlz-konsole-18.12.3-dev",
                "sha256:ba7816bf8f01cfea414140de5dae2223b00361a696177a9cf410ff61f20015ad!bin" => "7h7qgvs4kgzsn8a6rb273saxyqh4jxlz-konsole-18.12.3-bin",

            ],
        }
    )]
    fn parse_realisation(#[case] value: &str, #[case] expected: Realisation) {
        let actual: Realisation = serde_json::from_str(value).unwrap();
        pretty_assertions::assert_eq!(actual, expected);
    }

    #[rstest]
    #[case(
        Realisation {
            id: "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad!out".parse().unwrap(),
            out_path: "7h7qgvs4kgzsn8a6rb273saxyqh4jxlz-konsole-18.12.3".parse().unwrap(),
            signatures: set!["cache.nixos.org-1:0CpHca+06TwFp9VkMyz5OaphT3E8mnS+1SWymYlvFaghKSYPCMQ66TS1XPAr1+y9rfQZPLaHrBjjnIRktE/nAA=="],
            dependent_realisations: btree_map![
                "sha256:ba7816bf8f01cfea414140de5dae2223b00361a496177a9cf410ff61f20015ad!dev" => "7h7qgvs4kgzsn8a6rb273saxyqh4jxlz-konsole-18.12.3-dev",
                "sha256:ba7816bf8f01cfea414140de5dae2223b00361a696177a9cf410ff61f20015ad!bin" => "7h7qgvs4kgzsn8a6rb273saxyqh4jxlz-konsole-18.12.3-bin",

            ],
        },
        "{\"id\":\"sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad!out\",\"outPath\":\"7h7qgvs4kgzsn8a6rb273saxyqh4jxlz-konsole-18.12.3\",\"signatures\":[\"cache.nixos.org-1:0CpHca+06TwFp9VkMyz5OaphT3E8mnS+1SWymYlvFaghKSYPCMQ66TS1XPAr1+y9rfQZPLaHrBjjnIRktE/nAA==\"],\"dependentRealisations\":{\"sha256:ba7816bf8f01cfea414140de5dae2223b00361a496177a9cf410ff61f20015ad!dev\":\"7h7qgvs4kgzsn8a6rb273saxyqh4jxlz-konsole-18.12.3-dev\",\"sha256:ba7816bf8f01cfea414140de5dae2223b00361a696177a9cf410ff61f20015ad!bin\":\"7h7qgvs4kgzsn8a6rb273saxyqh4jxlz-konsole-18.12.3-bin\"}}",
    )]
    fn write_realisation(#[case] value: Realisation, #[case] expected: &str) {
        let actual = serde_json::to_string(&value).unwrap();
        pretty_assertions::assert_eq!(actual, expected);
    }

    // TODO: These protocol serialization tests belong in harmonia-protocol, not store-core
    // Store-core should be protocol-agnostic. The JSON serialization is already tested above.
    // Keeping these commented out until they can be moved to harmonia-protocol test suite.

    // #[tokio::test]
    // #[rstest]
    // #[case(
    //     Realisation {
    //         id: "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad!out".parse().unwrap(),
    //         out_path: "7h7qgvs4kgzsn8a6rb273saxyqh4jxlz-konsole-18.12.3".parse().unwrap(),
    //         signatures: set!["cache.nixos.org-1:0CpHca+06TwFp9VkMyz5OaphT3E8mnS+1SWymYlvFaghKSYPCMQ66TS1XPAr1+y9rfQZPLaHrBjjnIRktE/nAA=="],
    //         dependent_realisations: btree_map![
    //             "sha256:ba7816bf8f01cfea414140de5dae2223b00361a496177a9cf410ff61f20015ad!dev" => "7h7qgvs4kgzsn8a6rb273saxyqh4jxlz-konsole-18.12.3-dev",
    //             "sha256:ba7816bf8f01cfea414140de5dae2223b00361a696177a9cf410ff61f20015ad!bin" => "7h7qgvs4kgzsn8a6rb273saxyqh4jxlz-konsole-18.12.3-bin",

    //         ],
    //     },
    //     "{\"id\":\"sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad!out\",\"outPath\":\"7h7qgvs4kgzsn8a6rb273saxyqh4jxlz-konsole-18.12.3\",\"signatures\":[\"cache.nixos.org-1:0CpHca+06TwFp9VkMyz5OaphT3E8mnS+1SWymYlvFaghKSYPCMQ66TS1XPAr1+y9rfQZPLaHrBjjnIRktE/nAA==\"],\"dependentRealisations\":{\"sha256:ba7816bf8f01cfea414140de5dae2223b00361a496177a9cf410ff61f20015ad!dev\":\"7h7qgvs4kgzsn8a6rb273saxyqh4jxlz-konsole-18.12.3-dev\",\"sha256:ba7816bf8f01cfea414140de5dae2223b00361a696177a9cf410ff61f20015ad!bin\":\"7h7qgvs4kgzsn8a6rb273saxyqh4jxlz-konsole-18.12.3-bin\"}}",
    // )]
    // async fn nix_write_realisation(#[case] value: Realisation, #[case] expected: &str) {
    //     use harmonia_protocol::daemon::ser::NixWrite as _;

    //     let mut mock = harmonia_protocol::daemon::ser::mock::Builder::new()
    //         .write_slice(expected.as_bytes())
    //         .build();
    //     mock.write_value(&value).await.unwrap();
    // }

    // #[tokio::test]
    // #[rstest]
    // #[case(
    //     Realisation {
    //         id: "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad!out".parse().unwrap(),
    //         out_path: "7h7qgvs4kgzsn8a6rb273saxyqh4jxlz-konsole-18.12.3".parse().unwrap(),
    //         signatures: set!["cache.nixos.org-1:0CpHca+06TwFp9VkMyz5OaphT3E8mnS+1SWymYlvFaghKSYPCMQ66TS1XPAr1+y9rfQZPLaHrBjjnIRktE/nAA=="],
    //         dependent_realisations: btree_map![
    //             "sha256:ba7816bf8f01cfea414140de5dae2223b00361a496177a9cf410ff61f20015ad!dev" => "7h7qgvs4kgzsn8a6rb273saxyqh4jxlz-konsole-18.12.3-dev",
    //             "sha256:ba7816bf8f01cfea414140de5dae2223b00361a696177a9cf410ff61f20015ad!bin" => "7h7qgvs4kgzsn8a6rb273saxyqh4jxlz-konsole-18.12.3-bin",

    //         ],
    //     },
    //     "{\"id\":\"sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad!out\",\"outPath\":\"7h7qgvs4kgzsn8a6rb273saxyqh4jxlz-konsole-18.12.3\",\"signatures\":[\"cache.nixos.org-1:0CpHca+06TwFp9VkMyz5OaphT3E8mnS+1SWymYlvFaghKSYPCMQ66TS1XPAr1+y9rfQZPLaHrBjjnIRktE/nAA==\"],\"dependentRealisations\":{\"sha256:ba7816bf8f01cfea414140de5dae2223b00361a496177a9cf410ff61f20015ad!dev\":\"7h7qgvs4kgzsn8a6rb273saxyqh4jxlz-konsole-18.12.3-dev\",\"sha256:ba7816bf8f01cfea414140de5dae2223b00361a696177a9cf410ff61f20015ad!bin\":\"7h7qgvs4kgzsn8a6rb273saxyqh4jxlz-konsole-18.12.3-bin\"}}",
    // )]
    // async fn nix_read_realisation(#[case] expected: Realisation, #[case] value: &str) {
    //     use harmonia_protocol::daemon::de::NixRead as _;

    //     let mut mock = harmonia_protocol::daemon::de::mock::Builder::new()
    //         .read_slice(value.as_bytes())
    //         .build();
    //     let actual: Realisation = mock.read_value().await.unwrap();
    //     pretty_assertions::assert_eq!(actual, expected);
    // }
}
