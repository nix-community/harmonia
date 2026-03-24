use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::derived_path::OutputName;
use crate::signature::Signature;
use crate::store_path::{ParseStorePathError, StorePath, StorePathNameError};

#[derive(Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DrvOutput {
    pub drv_path: StorePath,
    pub output_name: OutputName,
}

impl fmt::Display for DrvOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}^{}", self.drv_path, self.output_name)
    }
}

#[derive(Debug, PartialEq, Clone, Error)]
pub enum ParseDrvOutputError {
    #[error("derivation output {0}")]
    StorePath(
        #[from]
        #[source]
        ParseStorePathError,
    ),
    #[error("derivation output has {0}")]
    OutputName(
        #[from]
        #[source]
        StorePathNameError,
    ),
    #[error("missing '^' in derivation output '{0}'")]
    InvalidDerivationOutputId(String),
}

impl FromStr for DrvOutput {
    type Err = ParseDrvOutputError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some((drv_path_s, output_name_s)) = s.split_once('^') {
            let drv_path = drv_path_s.parse()?;
            let output_name = output_name_s.parse()?;
            Ok(DrvOutput {
                drv_path,
                output_name,
            })
        } else {
            Err(ParseDrvOutputError::InvalidDerivationOutputId(s.into()))
        }
    }
}

/// The value part of a realisation: output path and signatures.
///
/// Used in contexts where the key (drv_path + output_name) is provided
/// externally, such as `BuildResult.built_outputs`.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnkeyedRealisation {
    pub out_path: StorePath,
    #[serde(default)]
    pub signatures: BTreeSet<Signature>,
}

/// A realisation maps a derivation output to its built store path.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Serialize, Deserialize)]
pub struct Realisation {
    pub key: DrvOutput,
    pub value: UnkeyedRealisation,
}

impl UnkeyedRealisation {
    /// Compute the fingerprint used for signing.
    ///
    /// Constructs a full `Realisation` JSON (with the given key), then strips
    /// the `signatures` field from the `value` object — matching upstream Nix.
    #[must_use]
    pub fn fingerprint(&self, key: &DrvOutput) -> String {
        let r = Realisation {
            key: key.clone(),
            value: self.clone(),
        };
        let mut json = serde_json::to_value(&r).expect("Realisation serialization cannot fail");
        json.get_mut("value")
            .and_then(|v| v.as_object_mut())
            .expect("Realisation must have value object")
            .remove("signatures");
        json.to_string()
    }

    /// Sign this realisation, returning the signature without modifying `self`.
    #[must_use]
    pub fn sign(&self, key: &DrvOutput, signer: &crate::signature::SecretKey) -> Signature {
        signer.sign(self.fingerprint(key).as_bytes())
    }

    /// Sign this realisation with the given secret keys, adding the resulting
    /// signatures to the `signatures` set.
    pub fn sign_mut(&mut self, key: &DrvOutput, keys: &[crate::signature::SecretKey]) {
        for k in keys {
            self.signatures.insert(self.sign(key, k));
        }
    }
}

#[cfg(any(test, feature = "test"))]
pub mod arbitrary {
    use crate::signature::proptests::arb_signatures;

    use super::*;
    use ::proptest::prelude::*;

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
            drv_path in any::<StorePath>(),
            output_name in any::<OutputName>(),
        ) -> DrvOutput
        {
            DrvOutput { drv_path, output_name }
        }
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
            drv_path in any::<StorePath>(),
            output_name in any::<OutputName>(),
            out_path in any::<StorePath>(),
            signatures in arb_signatures(),
        ) -> Realisation
        {
            Realisation {
                key: DrvOutput { drv_path, output_name },
                value: UnkeyedRealisation { out_path, signatures },
            }
        }
    }
}

#[cfg(test)]
mod unittests {
    use rstest::rstest;

    use crate::derived_path::OutputName;
    use crate::set;

    use super::{DrvOutput, Realisation, UnkeyedRealisation};

    #[rstest]
    #[case("g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-bar.drv^out", DrvOutput {
        drv_path: "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-bar.drv".parse().unwrap(),
        output_name: OutputName::default(),
    })]
    #[case("g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-bar.drv^out_put", DrvOutput {
        drv_path: "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-bar.drv".parse().unwrap(),
        output_name: "out_put".parse().unwrap(),
    })]
    fn parse_drv_output(#[case] value: &str, #[case] expected: DrvOutput) {
        let actual: DrvOutput = value.parse().unwrap();
        assert_eq!(actual, expected);
    }

    #[rstest]
    #[should_panic = "missing '^' in derivation output 'g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-bar.drv'"]
    #[case("g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-bar.drv")]
    #[should_panic = "derivation output has invalid name symbol '{' at position 3"]
    #[case("g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-bar.drv^out{put")]
    fn parse_drv_output_failure(#[case] value: &str) {
        let actual = value.parse::<DrvOutput>().unwrap_err();
        panic!("{actual}");
    }

    #[rstest]
    #[case(DrvOutput {
        drv_path: "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-bar.drv".parse().unwrap(),
        output_name: OutputName::default(),
    }, "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-bar.drv^out")]
    fn display_drv_output(#[case] value: DrvOutput, #[case] expected: &str) {
        assert_eq!(value.to_string(), expected);
    }

    #[rstest]
    #[case(
        Realisation {
            key: DrvOutput {
                drv_path: "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-bar.drv".parse().unwrap(),
                output_name: OutputName::default(),
            },
            value: UnkeyedRealisation {
                out_path: "7h7qgvs4kgzsn8a6rb273saxyqh4jxlz-konsole-18.12.3".parse().unwrap(),
                signatures: set!["cache.nixos.org-1:0CpHca+06TwFp9VkMyz5OaphT3E8mnS+1SWymYlvFaghKSYPCMQ66TS1XPAr1+y9rfQZPLaHrBjjnIRktE/nAA=="],
            },
        },
    )]
    fn parse_realisation(#[case] expected: Realisation) {
        // Round-trip: serialize then deserialize & verify equality
        let json = serde_json::to_string(&expected).unwrap();
        let parsed: Realisation = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, expected);
    }

    #[rstest]
    #[case(
        Realisation {
            key: DrvOutput {
                drv_path: "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-bar.drv".parse().unwrap(),
                output_name: OutputName::default(),
            },
            value: UnkeyedRealisation {
                out_path: "7h7qgvs4kgzsn8a6rb273saxyqh4jxlz-konsole-18.12.3".parse().unwrap(),
                signatures: set!["cache.nixos.org-1:0CpHca+06TwFp9VkMyz5OaphT3E8mnS+1SWymYlvFaghKSYPCMQ66TS1XPAr1+y9rfQZPLaHrBjjnIRktE/nAA=="],
            },
        },
    )]
    fn write_realisation(#[case] value: Realisation) {
        // Round-trip: serialize then deserialize & verify equality
        let json = serde_json::to_string(&value).unwrap();
        let parsed: Realisation = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, value);
    }

    #[test]
    fn fingerprint_strips_signatures() {
        let r = Realisation {
            key: DrvOutput {
                drv_path: "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-bar.drv".parse().unwrap(),
                output_name: "baz".parse().unwrap(),
            },
            value: UnkeyedRealisation {
                out_path: "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-foo".parse().unwrap(),
                signatures: set![
                    "asdf:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA==",
                    "qwer:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=="
                ],
            },
        };

        let fp = r.value.fingerprint(&r.key);
        let parsed: serde_json::Value = serde_json::from_str(&fp).unwrap();
        let value = parsed.get("value").expect("fingerprint must have value");
        assert!(
            value.get("signatures").is_none(),
            "signatures must be stripped from value"
        );
    }

    #[test]
    fn sign_adds_signature() {
        let mut r = Realisation {
            key: DrvOutput {
                drv_path: "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-bar.drv".parse().unwrap(),
                output_name: "baz".parse().unwrap(),
            },
            value: UnkeyedRealisation {
                out_path: "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-foo".parse().unwrap(),
                signatures: Default::default(),
            },
        };
        assert!(r.value.signatures.is_empty());
        let rng = ring::rand::SystemRandom::new();
        let sk = crate::signature::SecretKey::generate("test-key".to_string(), &rng).unwrap();
        let key = r.key.clone();
        r.value.sign_mut(&key, std::slice::from_ref(&sk));
        assert_eq!(r.value.signatures.len(), 1);
        assert_eq!(r.value.signatures.iter().next().unwrap().name(), "test-key");
    }
}
