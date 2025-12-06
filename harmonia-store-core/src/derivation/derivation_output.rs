use std::{collections::BTreeMap, fmt};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::derived_path::OutputName;
use crate::store_path::ContentAddressMethod;
use crate::store_path::ContentAddressMethodAlgorithm;
use crate::store_path::{ContentAddress, StoreDir, StorePath, StorePathName, StorePathNameError};
use harmonia_utils_hash::Hash;

/// Helper for formatting output path names.
///
/// Formats as just the derivation name for the default "out" output,
/// or as "name-output" for other outputs.
pub struct OutputPathName<'b> {
    pub drv_name: &'b StorePathName,
    pub output_name: &'b OutputName,
}

impl fmt::Display for OutputPathName<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.output_name.is_default() {
            write!(f, "{}", self.drv_name)
        } else {
            write!(f, "{}-{}", self.drv_name, self.output_name)
        }
    }
}

/// Helper struct for JSON serialization/deserialization of DerivationOutput
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawDerivationOutput {
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<StorePath>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hash: Option<Hash>,
    #[serde(skip_serializing_if = "Option::is_none")]
    method: Option<ContentAddressMethod>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hash_algo: Option<harmonia_utils_hash::Algorithm>,
    #[serde(default, skip_serializing_if = "is_false")]
    impure: bool,
}

fn is_false(b: &bool) -> bool {
    !b
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
pub enum DerivationOutput {
    InputAddressed(StorePath),
    CAFixed(ContentAddress),
    Deferred,
    CAFloating(ContentAddressMethodAlgorithm),
    Impure(ContentAddressMethodAlgorithm),
}

impl Serialize for DerivationOutput {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let raw = match self {
            DerivationOutput::InputAddressed(path) => RawDerivationOutput {
                path: Some(path.clone()),
                hash: None,
                method: None,
                hash_algo: None,
                impure: false,
            },
            DerivationOutput::CAFixed(ca) => RawDerivationOutput {
                path: None,
                hash: Some(ca.hash()),
                method: Some(ca.method()),
                hash_algo: None,
                impure: false,
            },
            DerivationOutput::Deferred => RawDerivationOutput {
                path: None,
                hash: None,
                method: None,
                hash_algo: None,
                impure: false,
            },
            DerivationOutput::CAFloating(ca_method_algo) => RawDerivationOutput {
                path: None,
                hash: None,
                method: Some(ca_method_algo.method()),
                hash_algo: Some(ca_method_algo.algorithm()),
                impure: false,
            },
            DerivationOutput::Impure(ca_method_algo) => RawDerivationOutput {
                path: None,
                hash: None,
                method: Some(ca_method_algo.method()),
                hash_algo: Some(ca_method_algo.algorithm()),
                impure: true,
            },
        };
        raw.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for DerivationOutput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de;

        let raw = RawDerivationOutput::deserialize(deserializer)?;

        // Determine variant based on which fields are present
        if let Some(path) = raw.path {
            // InputAddressed
            Ok(DerivationOutput::InputAddressed(path))
        } else if let Some(hash) = raw.hash {
            // CAFixed
            let method = raw
                .method
                .ok_or_else(|| de::Error::missing_field("method"))?;
            let ca = ContentAddress::from_hash(method, hash)
                .map_err(|e| de::Error::custom(format!("invalid content address: {}", e)))?;
            Ok(DerivationOutput::CAFixed(ca))
        } else if let Some(method) = raw.method {
            // CAFloating or Impure
            let algo = raw
                .hash_algo
                .ok_or_else(|| de::Error::missing_field("hashAlgo"))?;
            let ca_method_algo = match method {
                ContentAddressMethod::Text => ContentAddressMethodAlgorithm::Text,
                ContentAddressMethod::Flat => ContentAddressMethodAlgorithm::Flat(algo),
                ContentAddressMethod::Recursive => ContentAddressMethodAlgorithm::Recursive(algo),
            };
            if raw.impure {
                Ok(DerivationOutput::Impure(ca_method_algo))
            } else {
                Ok(DerivationOutput::CAFloating(ca_method_algo))
            }
        } else {
            // Deferred (empty object)
            Ok(DerivationOutput::Deferred)
        }
    }
}

impl DerivationOutput {
    pub fn path(
        &self,
        store_dir: &StoreDir,
        drv_name: &StorePathName,
        output_name: &OutputName,
    ) -> Result<Option<StorePath>, StorePathNameError> {
        match self {
            DerivationOutput::InputAddressed(store_path) => Ok(Some(store_path.clone())),
            DerivationOutput::CAFixed(ca) => {
                let name = OutputPathName {
                    drv_name,
                    output_name,
                }
                .to_string()
                .parse()?;
                Ok(Some(store_dir.make_store_path_from_ca(name, *ca)))
            }
            _ => Ok(None),
        }
    }
}

pub type DerivationOutputs = BTreeMap<OutputName, DerivationOutput>;

#[cfg(test)]
pub mod arbitrary {
    use super::*;
    use crate::test::arbitrary::helpers::Union;
    use ::proptest::prelude::*;
    use ::proptest::sample::SizeRange;
    use harmonia_utils_hash as hash;

    pub fn arb_derivation_outputs(
        size: impl Into<SizeRange>,
    ) -> impl Strategy<Value = DerivationOutputs> {
        use DerivationOutput::*;
        let size = size.into();
        let size2 = size.clone();
        //InputAddressed
        let input = prop::collection::btree_map(
            any::<OutputName>(),
            arb_derivation_output_input_addressed(),
            size.clone(),
        )
        .boxed();
        // CAFixed
        let fixed = arb_derivation_output_fixed()
            .prop_map(|ca| {
                let mut ret = BTreeMap::new();
                let name = OutputName::default();
                ret.insert(name, ca);
                ret
            })
            .boxed();
        // Deferred
        let deferred =
            prop::collection::btree_map(any::<OutputName>(), Just(Deferred), size.clone()).boxed();

        #[cfg_attr(test, allow(unused_mut))]
        let mut ret = Union::new([input, fixed, deferred]);
        {
            // CAFloating
            ret = ret.or(any::<hash::Algorithm>()
                .prop_flat_map(move |hash_type| {
                    prop::collection::btree_map(
                        any::<OutputName>(),
                        arb_derivation_output_floating(Just(hash_type)),
                        size2.clone(),
                    )
                })
                .boxed());
        }
        {
            // Impure
            ret = ret.or(prop::collection::btree_map(
                any::<OutputName>(),
                arb_derivation_output_impure(),
                size.clone(),
            )
            .boxed());
        }
        ret
    }

    impl Arbitrary for DerivationOutput {
        type Parameters = ();
        type Strategy = BoxedStrategy<DerivationOutput>;

        fn arbitrary_with(_args: Self::Parameters) -> Self::Strategy {
            arb_derivation_output().boxed()
        }
    }

    pub fn arb_derivation_output_input_addressed() -> impl Strategy<Value = DerivationOutput> {
        any::<StorePath>().prop_map(DerivationOutput::InputAddressed)
    }

    pub fn arb_derivation_output_fixed() -> impl Strategy<Value = DerivationOutput> {
        prop_oneof![
            any::<hash::Hash>().prop_map(|h| DerivationOutput::CAFixed(ContentAddress::Flat(h))),
            any::<hash::Hash>()
                .prop_map(|h| DerivationOutput::CAFixed(ContentAddress::Recursive(h)))
        ]
    }

    pub fn arb_derivation_output_impure() -> impl Strategy<Value = DerivationOutput> {
        any::<ContentAddressMethodAlgorithm>() // This works with derive(Arbitrary)
            .prop_map(DerivationOutput::Impure)
    }

    pub fn arb_derivation_output_floating<H>(
        hash_type: H,
    ) -> impl Strategy<Value = DerivationOutput>
    where
        H: Strategy<Value = hash::Algorithm> + Clone,
    {
        prop_oneof![
            1 => Just(ContentAddressMethodAlgorithm::Text),
            2 => hash_type.clone().prop_map(ContentAddressMethodAlgorithm::Flat),
            2 => hash_type.prop_map(ContentAddressMethodAlgorithm::Recursive),
        ]
        .prop_map(DerivationOutput::CAFloating)
    }

    pub fn arb_derivation_output() -> impl Strategy<Value = DerivationOutput> {
        use DerivationOutput::*;
        prop_oneof![
            arb_derivation_output_input_addressed(),
            arb_derivation_output_fixed(),
            arb_derivation_output_floating(any::<hash::Algorithm>()),
            Just(Deferred),
            arb_derivation_output_impure(),
        ]
    }
}

#[cfg(test)]
mod unittests {
    use rstest::rstest;

    use super::DerivationOutput;
    use crate::derived_path::OutputName;
    use crate::store_path::{StorePath, StorePathName, StorePathNameError};

    #[rstest]
    #[case::deffered(DerivationOutput::Deferred, "a", "a", Ok(None))]
    #[case::input(DerivationOutput::InputAddressed("00000000000000000000000000000000-_".parse().unwrap()), "a", "a", Ok(Some("00000000000000000000000000000000-_".parse().unwrap())))]
    #[case::fixed_flat(DerivationOutput::CAFixed("fixed:sha256:248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1".parse().unwrap()), "konsole-18.12.3", "out", Ok(Some("g9ngnw4w5vr9y3xkb7k2awl3mp95abrb-konsole-18.12.3".parse().unwrap())))]
    #[case::fixed_sha1(DerivationOutput::CAFixed("fixed:r:sha1:84983e441c3bd26ebaae4aa1f95129e5e54670f1".parse().unwrap()), "konsole-18.12.3", "out", Ok(Some("ag0y7g6rci9zsdz9nxcq5l1qllx3r99x-konsole-18.12.3".parse().unwrap())))]
    #[case::fixed_source(DerivationOutput::CAFixed("fixed:r:sha256:248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1".parse().unwrap()), "konsole-18.12.3", "out", Ok(Some("1w01xxn8f7s9s4n65ry6rwd7x9awf04s-konsole-18.12.3".parse().unwrap())))]
    fn test_path(
        #[case] output: DerivationOutput,
        #[case] drv_name: StorePathName,
        #[case] output_name: OutputName,
        #[case] path: Result<Option<StorePath>, StorePathNameError>,
    ) {
        let store_dir = Default::default();
        assert_eq!(path, output.path(&store_dir, &drv_name, &output_name))
    }
}
