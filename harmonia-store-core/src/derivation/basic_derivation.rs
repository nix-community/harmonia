use std::collections::{BTreeMap, BTreeSet};

#[cfg(test)]
use proptest::prelude::{Arbitrary, BoxedStrategy};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::ByteString;
use crate::derived_path::SingleDerivedPath;
use crate::store_path::{StorePathName, StorePathSet};

use super::{DerivationInputs, DerivationOutputs};

/// Structured attributes for a derivation.
///
/// When present, derivation attributes are passed to the builder as a JSON object
/// via the `__json` environment variable, rather than individual environment variables.
/// This allows passing complex data structures (arrays, nested objects, etc.) to builders.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StructuredAttrs {
    /// The structured attributes as a JSON object.
    ///
    /// Note: The `env` map must not contain the key `__json` when structured attrs are used,
    /// as that key is reserved for encoding the structured attributes themselves.
    pub attrs: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DerivationT<Inputs> {
    /// The name of the derivation
    pub name: StorePathName,
    pub outputs: DerivationOutputs,
    pub inputs: Inputs,
    pub platform: ByteString,
    pub builder: ByteString,
    pub args: Vec<ByteString>,
    /// Environment variables passed to the builder.
    ///
    /// Note: Must not contain the key `__json` as that key is reserved
    /// for encoding structured attributes.
    pub env: BTreeMap<ByteString, ByteString>,
    /// Optional structured attributes.
    ///
    /// When present, attributes are passed to the builder as JSON via the `__json`
    /// environment variable instead of individual environment variables.
    pub structured_attrs: Option<StructuredAttrs>,
}

fn default_version() -> u32 {
    4
}

pub type BasicDerivation = DerivationT<StorePathSet>;
pub type Derivation = DerivationT<BTreeSet<SingleDerivedPath>>;

#[cfg(test)]
pub mod arbitrary {
    use super::*;
    use crate::{
        derivation::derivation_output::arbitrary::arb_derivation_outputs,
        test::arbitrary::arb_byte_string,
    };
    use ::proptest::prelude::*;
    use proptest::sample::SizeRange;

    impl Arbitrary for BasicDerivation {
        type Parameters = ();
        type Strategy = BoxedStrategy<BasicDerivation>;

        fn arbitrary_with(_args: Self::Parameters) -> Self::Strategy {
            arb_basic_derivation().boxed()
        }
    }

    prop_compose! {
        pub fn arb_basic_derivation()
        (
            name in any::<StorePathName>(),
            outputs in arb_derivation_outputs(1..15),
            inputs in any::<StorePathSet>(),
            platform in arb_byte_string(),
            builder in arb_byte_string(),
            args in proptest::collection::vec(arb_byte_string(), SizeRange::default()),
            env in proptest::collection::btree_map(arb_byte_string(), arb_byte_string(), SizeRange::default()),
            structured_attrs in proptest::option::of(arb_structured_attrs())
        ) -> BasicDerivation
        {
            DerivationT {
                name,
                outputs,
                inputs,
                platform,
                builder,
                args,
                env,
                structured_attrs,
            }
        }
    }

    fn arb_structured_attrs() -> impl Strategy<Value = StructuredAttrs> {
        // Generate a simple JSON object with string keys and various JSON values
        proptest::collection::btree_map(
            any::<String>(),
            prop_oneof![
                any::<String>().prop_map(serde_json::Value::String),
                any::<i64>().prop_map(|i| serde_json::Value::Number(i.into())),
                any::<bool>().prop_map(serde_json::Value::Bool),
                Just(serde_json::Value::Null),
            ],
            0..10,
        )
        .prop_map(|map| {
            let mut attrs = serde_json::Map::new();
            for (k, v) in map {
                attrs.insert(k, v);
            }
            StructuredAttrs { attrs }
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DerivationHelper {
    name: String,
    outputs: DerivationOutputs,
    inputs: DerivationInputs,
    #[serde(rename = "system")]
    platform: String,
    builder: String,
    args: Vec<String>,
    env: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    structured_attrs: Option<StructuredAttrs>,
    #[serde(default = "default_version")]
    version: u32,
}

impl Serialize for Derivation {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Convert BTreeSet<SingleDerivedPath> to DerivationInputs for serialization
        let inputs = DerivationInputs::from(&self.inputs);

        // Serialize ByteString fields as strings
        let platform_str = std::str::from_utf8(&self.platform)
            .map_err(|e| serde::ser::Error::custom(format!("invalid UTF-8 in platform: {}", e)))?;

        let builder_str = std::str::from_utf8(&self.builder)
            .map_err(|e| serde::ser::Error::custom(format!("invalid UTF-8 in builder: {}", e)))?;

        let args_strs: Result<Vec<String>, _> = self
            .args
            .iter()
            .map(|b| {
                std::str::from_utf8(b)
                    .map(|s| s.to_string())
                    .map_err(|e| serde::ser::Error::custom(format!("invalid UTF-8 in args: {}", e)))
            })
            .collect();
        let args_strs = args_strs?;

        // Serialize env map
        let env_map: Result<BTreeMap<String, String>, _> = self
            .env
            .iter()
            .map(|(k, v)| {
                Ok((
                    std::str::from_utf8(k)?.to_string(),
                    std::str::from_utf8(v)?.to_string(),
                ))
            })
            .collect::<Result<_, std::str::Utf8Error>>()
            .map_err(|e| serde::ser::Error::custom(format!("invalid UTF-8 in env: {}", e)));
        let env_map = env_map?;

        let helper = DerivationHelper {
            name: self.name.to_string(),
            outputs: self.outputs.clone(),
            inputs,
            platform: platform_str.to_string(),
            builder: builder_str.to_string(),
            args: args_strs,
            env: env_map,
            structured_attrs: self.structured_attrs.clone(),
            version: default_version(),
        };

        helper.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Derivation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let helper = DerivationHelper::deserialize(deserializer)?;

        // Assert version is 4
        if helper.version != 4 {
            return Err(serde::de::Error::custom(format!(
                "unsupported derivation version: {}, expected 4",
                helper.version
            )));
        }

        // Convert DerivationInputs to BTreeSet<SingleDerivedPath>
        let inputs = BTreeSet::from(&helper.inputs);

        Ok(Derivation {
            name: helper
                .name
                .parse()
                .map_err(|e| serde::de::Error::custom(format!("invalid derivation name: {}", e)))?,
            outputs: helper.outputs,
            inputs,
            platform: ByteString::from(helper.platform),
            builder: ByteString::from(helper.builder),
            args: helper.args.into_iter().map(ByteString::from).collect(),
            env: helper
                .env
                .into_iter()
                .map(|(k, v)| (ByteString::from(k), ByteString::from(v)))
                .collect(),
            structured_attrs: helper.structured_attrs,
        })
    }
}
