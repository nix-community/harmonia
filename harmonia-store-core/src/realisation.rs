use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;

use derive_more::Display;
use serde::{Deserialize, Serialize};
use serde_with::{DeserializeFromStr, SerializeDisplay};
use thiserror::Error;

use crate::derived_path::OutputName;
use crate::signature::Signature;
use crate::store_path::{StorePath, StorePathNameError};
use harmonia_utils_hash::Hash;
use harmonia_utils_hash::fmt::Any;

/// Identifies a specific output of a derivation.
///
/// String form: `sha256:<hex>!<output_name>`, where the hash is the
/// "hash modulo" of the derivation.
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
    pub drv_hash: harmonia_utils_hash::Hash,
    pub output_name: OutputName,
}

#[derive(Debug, PartialEq, Clone, Error)]
pub enum ParseDrvOutputError {
    #[error("derivation output {0}")]
    Hash(
        #[from]
        #[source]
        harmonia_utils_hash::fmt::ParseHashError,
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
    pub out_path: StorePath,
    pub signatures: BTreeSet<Signature>,
    /// Always empty. Nix hardcodes `"dependentRealisations": {}` for backwards compat.
    #[serde(default)]
    pub dependent_realisations: BTreeMap<DrvOutput, StorePath>,
}

impl Realisation {
    /// Compute the fingerprint used for signing.
    #[must_use]
    pub fn fingerprint(&self) -> String {
        let mut json = serde_json::to_value(self).expect("Realisation serialization cannot fail");
        json.as_object_mut()
            .expect("Realisation must serialize as object")
            .remove("signatures");
        json.to_string()
    }

    /// Sign this realisation with the given secret keys, adding the resulting
    /// signatures to the `signatures` set.
    pub fn sign(&mut self, keys: &[crate::signature::SecretKey]) {
        let fp = self.fingerprint();
        for key in keys {
            self.signatures.insert(key.sign(fp.as_bytes()));
        }
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
            drv_hash in any::<harmonia_utils_hash::Hash>(),
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
        ) -> Realisation
        {
            Realisation { id, out_path, signatures, dependent_realisations: Default::default() }
        }
    }
}

#[cfg(test)]
mod unittests {
    use rstest::rstest;

    use crate::derived_path::OutputName;
    use crate::set;

    use harmonia_utils_hash::Hash;
    use harmonia_utils_hash::fmt::Any;

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
        "{\"dependentRealisations\":{},\"id\":\"sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad!out\",\"outPath\":\"7h7qgvs4kgzsn8a6rb273saxyqh4jxlz-konsole-18.12.3\",\"signatures\":[\"cache.nixos.org-1:0CpHca+06TwFp9VkMyz5OaphT3E8mnS+1SWymYlvFaghKSYPCMQ66TS1XPAr1+y9rfQZPLaHrBjjnIRktE/nAA==\"]}",
        Realisation {
            id: DrvOutput {
                drv_hash: "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".parse::<Any<Hash>>().unwrap().into_hash(),
                output_name: OutputName::default(),
            },
            out_path: "7h7qgvs4kgzsn8a6rb273saxyqh4jxlz-konsole-18.12.3".parse().unwrap(),
            signatures: set!["cache.nixos.org-1:0CpHca+06TwFp9VkMyz5OaphT3E8mnS+1SWymYlvFaghKSYPCMQ66TS1XPAr1+y9rfQZPLaHrBjjnIRktE/nAA=="],
            dependent_realisations: Default::default(),
        },
    )]
    fn parse_realisation(#[case] value: &str, #[case] expected: Realisation) {
        let actual: Realisation = serde_json::from_str(value).unwrap();
        assert_eq!(actual, expected);
    }

    #[rstest]
    #[case(
        Realisation {
            id: DrvOutput {
                drv_hash: "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".parse::<Any<Hash>>().unwrap().into_hash(),
                output_name: OutputName::default(),
            },
            out_path: "7h7qgvs4kgzsn8a6rb273saxyqh4jxlz-konsole-18.12.3".parse().unwrap(),
            signatures: set!["cache.nixos.org-1:0CpHca+06TwFp9VkMyz5OaphT3E8mnS+1SWymYlvFaghKSYPCMQ66TS1XPAr1+y9rfQZPLaHrBjjnIRktE/nAA=="],
            dependent_realisations: Default::default(),
        },
    )]
    fn write_realisation(#[case] value: Realisation) {
        // Round-trip: serialize then deserialize & verify equality
        let json = serde_json::to_string(&value).unwrap();
        let parsed: Realisation = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, value);
        // Verify dependentRealisations is present in output (backwards compat)
        let raw = serde_json::from_str::<serde_json::Value>(&json).unwrap();
        assert!(raw.get("dependentRealisations").is_some());
    }

    #[test]
    fn fingerprint_strips_signatures() {
        let r = Realisation {
            id: DrvOutput {
                drv_hash: "sha256:15e3c560894cbb27085cf65b5a2ecb18488c999497f4531b6907a7581ce6d527"
                    .parse::<Any<Hash>>()
                    .unwrap()
                    .into_hash(),
                output_name: "baz".parse().unwrap(),
            },
            out_path: "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-foo".parse().unwrap(),
            signatures: set![
                "asdf:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA==",
                "qwer:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=="
            ],
            dependent_realisations: Default::default(),
        };

        let fp = r.fingerprint();
        let parsed: serde_json::Value = serde_json::from_str(&fp).unwrap();
        assert_eq!(
            parsed["id"],
            "sha256:15e3c560894cbb27085cf65b5a2ecb18488c999497f4531b6907a7581ce6d527!baz"
        );
        assert_eq!(parsed["outPath"], "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-foo");
        assert!(
            parsed.get("signatures").is_none(),
            "signatures must be stripped"
        );
    }

    #[test]
    fn sign_adds_signature() {
        let mut r = Realisation {
            id: DrvOutput {
                drv_hash: "sha256:15e3c560894cbb27085cf65b5a2ecb18488c999497f4531b6907a7581ce6d527"
                    .parse::<Any<Hash>>()
                    .unwrap()
                    .into_hash(),
                output_name: "baz".parse().unwrap(),
            },
            out_path: "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-foo".parse().unwrap(),
            signatures: Default::default(),
            dependent_realisations: Default::default(),
        };
        assert!(r.signatures.is_empty());
        let rng = ring::rand::SystemRandom::new();
        let sk = crate::signature::SecretKey::generate("test-key".to_string(), &rng).unwrap();
        r.sign(&[sk]);
        assert_eq!(r.signatures.len(), 1);
        assert_eq!(r.signatures.iter().next().unwrap().name(), "test-key");
    }
}
