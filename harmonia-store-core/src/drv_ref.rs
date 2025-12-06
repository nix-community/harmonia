//! DrvRef - a reference that can be either a self-output or an external path.

use std::hash::Hash;

use serde::{Deserialize, Serialize};

use crate::derived_path::OutputName;

/// A reference that can appear in output check constraints.
///
/// This can be either:
/// - A self-reference to an output of the current derivation (`{ "drvPath": "self", "output": "out" }`)
/// - A deriving path (store path or built output)
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DrvRef<Input> {
    /// Reference to an output of the current derivation being built.
    SelfOutput(OutputName),
    /// Reference to an external path.
    External(Input),
}

impl<Input: Serialize> Serialize for DrvRef<Input> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            DrvRef::SelfOutput(output) => {
                #[derive(Serialize)]
                #[serde(rename_all = "camelCase")]
                struct SelfRef<'a> {
                    drv_path: &'static str,
                    output: &'a OutputName,
                }
                SelfRef {
                    drv_path: "self",
                    output,
                }
                .serialize(serializer)
            }
            DrvRef::External(input) => input.serialize(serializer),
        }
    }
}

impl<'de, Input: Deserialize<'de>> Deserialize<'de> for DrvRef<Input> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::{self, MapAccess, Visitor};
        use std::fmt;
        use std::marker::PhantomData;

        struct DrvRefVisitor<Input>(PhantomData<Input>);

        impl<'de, Input: Deserialize<'de>> Visitor<'de> for DrvRefVisitor<Input> {
            type Value = DrvRef<Input>;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a DrvRef (self-reference object or external input)")
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                // External input as string (e.g., store path)
                let input = Input::deserialize(de::value::StrDeserializer::new(v))?;
                Ok(DrvRef::External(input))
            }

            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut json_map = serde_json::Map::new();

                while let Some(key) = map.next_key::<String>()? {
                    json_map.insert(key, map.next_value()?);
                }

                // Check if this is a self-reference (drvPath == "self")
                let self_ref = (|| {
                    if json_map.get("drvPath")?.as_str()? != "self" {
                        return None;
                    }
                    let output = json_map.get("output")?.as_str()?;
                    let output_name = output.parse().ok()?;
                    Some(DrvRef::SelfOutput(output_name))
                })();

                if let Some(self_ref) = self_ref {
                    return Ok(self_ref);
                }

                // Otherwise, deserialize the whole map as Input
                let input = Input::deserialize(serde_json::Value::Object(json_map))
                    .map_err(de::Error::custom)?;
                Ok(DrvRef::External(input))
            }
        }

        deserializer.deserialize_any(DrvRefVisitor(PhantomData))
    }
}
