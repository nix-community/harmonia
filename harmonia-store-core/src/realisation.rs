use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

use derive_more::Display;

use serde::{Deserialize, Serialize};
use serde_with::{DeserializeFromStr, SerializeDisplay};
use thiserror::Error;

use crate::derived_path::OutputName;
use crate::signature::{Signature, Structured};
use crate::store_path::{StorePath, StorePathNameError};
use harmonia_utils_hash::Hash;
use harmonia_utils_hash::fmt::Any;

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

/// The value part of a realisation: output path and signatures.
///
/// Used in contexts where the key (drv_path + output_name) is provided
/// externally, such as `BuildResult.built_outputs`.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnkeyedRealisation {
    pub out_path: StorePath,
    #[serde(default, deserialize_with = "deserialize_signatures")]
    pub signatures: BTreeSet<Signature>,
}

fn deserialize_signatures<'de, D>(deserializer: D) -> Result<BTreeSet<Signature>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Structured::<BTreeSet<Signature>>::deserialize(deserializer).map(|s| s.0)
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
pub struct Realisation {
    pub drv_path: StorePath,
    pub output_name: OutputName,
    pub out_path: StorePath,
    pub signatures: BTreeSet<Signature>,
}

impl Serialize for Realisation {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;

        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Key<'a> {
            drv_path: &'a StorePath,
            output_name: &'a OutputName,
        }

        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Value<'a> {
            out_path: &'a StorePath,
            signatures: &'a BTreeSet<Signature>,
        }

        let mut map = serializer.serialize_map(Some(2))?;
        map.serialize_entry(
            "key",
            &Key {
                drv_path: &self.drv_path,
                output_name: &self.output_name,
            },
        )?;
        map.serialize_entry(
            "value",
            &Value {
                out_path: &self.out_path,
                signatures: &self.signatures,
            },
        )?;
        map.end()
    }
}

impl<'de> Deserialize<'de> for Realisation {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de;

        struct RealisationVisitor;

        impl<'de> de::Visitor<'de> for RealisationVisitor {
            type Value = Realisation;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(r#"a realisation object with "key" and "value" fields"#)
            }

            fn visit_map<A: de::MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut key: Option<KeyRaw> = None;
                let mut value: Option<ValueRaw> = None;

                while let Some(k) = map.next_key::<&str>()? {
                    match k {
                        "key" => key = Some(map.next_value()?),
                        "value" => value = Some(map.next_value()?),
                        _ => {
                            map.next_value::<de::IgnoredAny>()?;
                        }
                    }
                }

                let key = key.ok_or_else(|| de::Error::missing_field("key"))?;
                let value = value.ok_or_else(|| de::Error::missing_field("value"))?;

                Ok(Realisation {
                    drv_path: key.drv_path,
                    output_name: key.output_name,
                    out_path: value.out_path,
                    signatures: value.signatures.0,
                })
            }
        }

        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct KeyRaw {
            drv_path: StorePath,
            output_name: OutputName,
        }

        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct ValueRaw {
            out_path: StorePath,
            #[serde(default)]
            signatures: Structured<BTreeSet<Signature>>,
        }

        deserializer.deserialize_map(RealisationVisitor)
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
            drv_hash in any::<harmonia_utils_hash::Hash>(),
            output_name in any::<OutputName>(),
        ) -> DrvOutput
        {
            DrvOutput { drv_hash, output_name }
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
                drv_path, output_name, out_path, signatures,
            }
        }
    }
}

#[cfg(test)]
mod unittests {
    use rstest::rstest;

    use crate::derived_path::OutputName;
    use harmonia_utils_hash::Hash;
    use harmonia_utils_hash::fmt::Any;

    use super::DrvOutput;

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
}
