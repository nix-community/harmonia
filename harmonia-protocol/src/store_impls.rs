// SPDX-FileCopyrightText: 2024 griff (original Nix.rs)
// SPDX-FileCopyrightText: 2025 Jörg Thalheim (Harmonia adaptation)
// SPDX-License-Identifier: EUPL-1.2 OR MIT

//! Hand-written NixSerialize/NixDeserialize implementations for harmonia-store-core types.
//!
//! These impls have custom serialization logic that can't be expressed with derives,
//! so they live here in the protocol layer instead of in store-core.
//!
//! This module breaks the circular dependency: store-core has no protocol knowledge,
//! but protocol can impl traits for external store-core types.

use std::str::FromStr;

use crate::daemon_wire::logger::RawLogMessageType;
use crate::de::{NixDeserialize, NixRead};
use crate::ser::{NixSerialize, NixWrite};
use harmonia_protocol_derive::{nix_deserialize_remote, nix_serialize_remote};
use harmonia_store_core::derivation::{BasicDerivation, DerivationOutput};
use harmonia_store_core::derived_path::{DerivedPath, LegacyDerivedPath, OutputName};
use harmonia_store_core::log::{Activity, ActivityResult, LogMessage, StopActivity};
use harmonia_store_core::realisation::Realisation;
use harmonia_store_core::store_path::{
    ContentAddress, ContentAddressMethodAlgorithm, StorePath, StorePathName,
};

// ========== BasicDerivation ==========

impl NixSerialize for (StorePath, BasicDerivation) {
    async fn serialize<W>(&self, mut writer: &mut W) -> Result<(), W::Error>
    where
        W: NixWrite,
    {
        let (drv_path, drv) = self;
        writer.write_value(drv_path).await?;
        writer.write_value(&drv.outputs.len()).await?;
        for (output_name, output) in drv.outputs.iter() {
            writer.write_value(output_name).await?;
            write_derivation_output(output, &drv.name, output_name, &mut writer).await?;
        }
        writer.write_value(&drv.inputs).await?;
        writer.write_value(&drv.platform).await?;
        writer.write_value(&drv.builder).await?;
        writer.write_value(&drv.args).await?;
        writer.write_value(&drv.env).await?;
        Ok(())
    }
}

impl NixDeserialize for (StorePath, BasicDerivation) {
    async fn try_deserialize<R>(reader: &mut R) -> Result<Option<Self>, R::Error>
    where
        R: ?Sized + NixRead + Send,
    {
        use harmonia_store_core::derivation::DerivationOutputs;

        // Try to read the drv path - if not present, return None
        if let Some(drv_path) = reader.try_read_value::<StorePath>().await? {
            let name = drv_path.name().clone();

            let outputs_len = reader.read_value::<usize>().await?;
            let mut outputs = DerivationOutputs::new();

            for _ in 0..outputs_len {
                let output_name = reader.read_value::<OutputName>().await?;
                let output = read_derivation_output(reader, &name, &output_name).await?;
                outputs.insert(output_name, output);
            }

            let inputs = reader.read_value().await?;
            let platform = reader.read_value().await?;
            let builder = reader.read_value().await?;
            let args = reader.read_value().await?;
            let env = reader.read_value().await?;

            Ok(Some((
                drv_path,
                BasicDerivation {
                    name,
                    outputs,
                    inputs,
                    platform,
                    builder,
                    args,
                    env,
                    structured_attrs: None, // TODO: Read from wire protocol if present
                },
            )))
        } else {
            Ok(None)
        }
    }
}

// ========== DerivationOutput ==========

fn output_path_name(drv_name: &StorePathName, output_name: &OutputName) -> String {
    if output_name.is_default() {
        drv_name.to_string()
    } else {
        format!("{}-{}", drv_name, output_name)
    }
}

async fn read_derivation_output<R>(
    reader: &mut R,
    _drv_name: &StorePathName,
    _output_name: &OutputName,
) -> Result<DerivationOutput, R::Error>
where
    R: ?Sized + NixRead + Send,
{
    use crate::de::Error;
    use harmonia_store_core::hash::fmt::Base32;

    let store_path_str = reader.read_value::<String>().await?;
    let method_str = reader.read_value::<String>().await?;
    let hash_str = reader.read_value::<String>().await?;

    if hash_str == "impure" {
        let algo = method_str.parse().map_err(R::Error::invalid_data)?;
        Ok(DerivationOutput::Impure(algo))
    } else if store_path_str.is_empty() && !method_str.is_empty() {
        let algo = method_str.parse().map_err(R::Error::invalid_data)?;
        Ok(DerivationOutput::CAFloating(algo))
    } else if store_path_str.is_empty() && method_str.is_empty() && hash_str.is_empty() {
        Ok(DerivationOutput::Deferred)
    } else if method_str.is_empty() && hash_str.is_empty() {
        let store_path = reader
            .store_dir()
            .parse(&store_path_str)
            .map_err(R::Error::invalid_data)?;
        Ok(DerivationOutput::InputAddressed(store_path))
    } else {
        // CAFixed
        let method_algo = method_str
            .parse::<ContentAddressMethodAlgorithm>()
            .map_err(R::Error::invalid_data)?;
        let hash = Base32::from_str(&hash_str).map_err(R::Error::invalid_data)?;
        let ca = ContentAddress::from_hash(method_algo.method(), *hash.as_hash())
            .map_err(R::Error::invalid_data)?;
        Ok(DerivationOutput::CAFixed(ca))
    }
}

async fn write_derivation_output<W>(
    output: &DerivationOutput,
    drv_name: &StorePathName,
    output_name: &OutputName,
    writer: &mut W,
) -> Result<(), W::Error>
where
    W: NixWrite,
{
    use crate::ser::Error;

    match output {
        DerivationOutput::InputAddressed(store_path) => {
            writer.write_value(store_path).await?;
            writer.write_value("").await?;
            writer.write_value("").await?;
        }
        DerivationOutput::CAFixed(ca) => {
            let name = output_path_name(drv_name, output_name)
                .to_string()
                .parse()
                .map_err(Error::unsupported_data)?;
            let path = writer.store_dir().make_store_path_from_ca(name, *ca);
            writer.write_value(&path).await?;
            writer.write_value(&ca.method_algorithm()).await?;
            writer.write_display(ca.hash().base32().bare()).await?;
        }
        DerivationOutput::Deferred => {
            writer.write_value("").await?;
            writer.write_value("").await?;
            writer.write_value("").await?;
        }
        DerivationOutput::CAFloating(algo) => {
            writer.write_value("").await?;
            writer.write_value(algo).await?;
            writer.write_value("").await?;
        }
        DerivationOutput::Impure(algo) => {
            writer.write_value("").await?;
            writer.write_value(algo).await?;
            writer.write_value("impure").await?;
        }
    }
    Ok(())
}

// ========== DerivedPath ==========

impl NixSerialize for DerivedPath {
    async fn serialize<W>(&self, writer: &mut W) -> Result<(), W::Error>
    where
        W: NixWrite,
    {
        let store_dir = writer.store_dir().clone();
        writer
            .write_display(store_dir.display(&self.to_legacy_format()))
            .await
    }
}

impl NixDeserialize for DerivedPath {
    async fn try_deserialize<R>(reader: &mut R) -> Result<Option<Self>, R::Error>
    where
        R: ?Sized + NixRead + Send,
    {
        use crate::de::Error;
        if let Some(s) = reader.try_read_value::<String>().await? {
            let legacy = reader
                .store_dir()
                .parse::<LegacyDerivedPath>(&s)
                .map_err(R::Error::invalid_data)?;
            Ok(Some(legacy.0))
        } else {
            Ok(None)
        }
    }
}

// ========== LogMessage ==========

impl NixSerialize for LogMessage {
    async fn serialize<W>(&self, writer: &mut W) -> Result<(), W::Error>
    where
        W: NixWrite,
    {
        match self {
            LogMessage::Message(msg) => {
                writer.write_value(&RawLogMessageType::Next).await?;
                writer.write_value(&msg.text).await?;
            }
            LogMessage::StartActivity(act) => {
                if writer.version().minor() >= 20 {
                    writer
                        .write_value(&RawLogMessageType::StartActivity)
                        .await?;
                    writer.write_value(act).await?;
                } else {
                    writer.write_value(&RawLogMessageType::Next).await?;
                    writer.write_value(&act.text).await?;
                }
            }
            LogMessage::StopActivity(act) => {
                if writer.version().minor() >= 20 {
                    writer.write_value(&RawLogMessageType::StopActivity).await?;
                    writer.write_value(&act.id).await?;
                }
            }
            LogMessage::Result(result) => {
                if writer.version().minor() >= 20 {
                    writer.write_value(&RawLogMessageType::Result).await?;
                    writer.write_value(result).await?;
                }
            }
        }
        Ok(())
    }
}

// ========== Activity ==========

impl NixSerialize for Activity {
    async fn serialize<W>(&self, writer: &mut W) -> Result<(), W::Error>
    where
        W: NixWrite,
    {
        writer.write_value(&self.id).await?;
        writer.write_value(&self.level).await?;
        writer.write_value(&self.activity_type).await?;
        writer.write_value(&self.text).await?;
        writer.write_value(&self.fields).await?;
        writer.write_value(&self.parent).await?;
        Ok(())
    }
}

impl NixDeserialize for Activity {
    async fn try_deserialize<R>(reader: &mut R) -> Result<Option<Self>, R::Error>
    where
        R: ?Sized + NixRead + Send,
    {
        if let Some(id) = reader.try_read_value::<u64>().await? {
            let level = reader.read_value().await?;
            let activity_type = reader.read_value().await?;
            let text = reader.read_value().await?;
            let fields = reader.read_value().await?;
            let parent = reader.read_value().await?;
            Ok(Some(Self {
                id,
                level,
                activity_type,
                text,
                fields,
                parent,
            }))
        } else {
            Ok(None)
        }
    }
}

// ========== ActivityResult ==========

impl NixSerialize for ActivityResult {
    async fn serialize<W>(&self, writer: &mut W) -> Result<(), W::Error>
    where
        W: NixWrite,
    {
        writer.write_value(&self.id).await?;
        writer.write_value(&self.result_type).await?;
        writer.write_value(&self.fields).await?;
        Ok(())
    }
}

impl NixDeserialize for ActivityResult {
    async fn try_deserialize<R>(reader: &mut R) -> Result<Option<Self>, R::Error>
    where
        R: ?Sized + NixRead + Send,
    {
        if let Some(id) = reader.try_read_value().await? {
            let result_type = reader.read_value().await?;
            let fields = reader.read_value().await?;
            Ok(Some(Self {
                fields,
                id,
                result_type,
            }))
        } else {
            Ok(None)
        }
    }
}

// ========== Realisation ==========

impl NixSerialize for Realisation {
    async fn serialize<W>(&self, writer: &mut W) -> Result<(), W::Error>
    where
        W: NixWrite,
    {
        use crate::ser::Error;
        let s = serde_json::to_string(&self).map_err(W::Error::custom)?;
        writer.write_slice(s.as_bytes()).await
    }
}

impl NixDeserialize for Realisation {
    async fn try_deserialize<R>(reader: &mut R) -> Result<Option<Self>, R::Error>
    where
        R: ?Sized + NixRead + Send,
    {
        use crate::de::Error;
        if let Some(buf) = reader.try_read_bytes().await? {
            Ok(Some(
                serde_json::from_slice(&buf).map_err(R::Error::custom)?,
            ))
        } else {
            Ok(None)
        }
    }
}

// ========== Simple derives for store-core types using remote macros ==========

// These types can use automatic derives, so we use the remote macros

// StorePath
nix_deserialize_remote!(
    #[nix(from_store_dir_str)]
    harmonia_store_core::store_path::StorePath
);
nix_serialize_remote!(
    #[nix(store_dir_display)]
    harmonia_store_core::store_path::StorePath
);

// Verbosity
nix_deserialize_remote!(
    #[nix(from = "u16")]
    harmonia_store_core::log::Verbosity
);
nix_serialize_remote!(
    #[nix(into = "u16")]
    harmonia_store_core::log::Verbosity
);

// ActivityType
nix_deserialize_remote!(
    #[nix(try_from = "u16")]
    harmonia_store_core::log::ActivityType
);
nix_serialize_remote!(
    #[nix(into = "u16")]
    harmonia_store_core::log::ActivityType
);

// ResultType
nix_deserialize_remote!(
    #[nix(try_from = "u16")]
    harmonia_store_core::log::ResultType
);
nix_serialize_remote!(
    #[nix(into = "u16")]
    harmonia_store_core::log::ResultType
);

// FieldType
nix_deserialize_remote!(
    #[nix(try_from = "u16")]
    harmonia_store_core::log::FieldType
);
nix_serialize_remote!(
    #[nix(into = "u16")]
    harmonia_store_core::log::FieldType
);

// StopActivity (simple struct - manual impl)
impl NixSerialize for StopActivity {
    async fn serialize<W>(&self, writer: &mut W) -> Result<(), W::Error>
    where
        W: NixWrite,
    {
        writer.write_value(&self.id).await
    }
}

impl NixDeserialize for StopActivity {
    async fn try_deserialize<R>(reader: &mut R) -> Result<Option<Self>, R::Error>
    where
        R: ?Sized + NixRead + Send,
    {
        if let Some(id) = reader.try_read_value::<u64>().await? {
            Ok(Some(Self { id }))
        } else {
            Ok(None)
        }
    }
}

// Field (tagged enum) - manual impls because remote macros don't support tags
use harmonia_store_core::log::{Field, FieldType};

impl NixSerialize for Field {
    async fn serialize<W>(&self, writer: &mut W) -> Result<(), W::Error>
    where
        W: NixWrite,
    {
        match self {
            Field::Int(v) => {
                writer.write_value(&FieldType::Int).await?;
                writer.write_value(v).await?;
            }
            Field::String(s) => {
                writer.write_value(&FieldType::String).await?;
                writer.write_value(s).await?;
            }
        }
        Ok(())
    }
}

impl NixDeserialize for Field {
    async fn try_deserialize<R>(reader: &mut R) -> Result<Option<Self>, R::Error>
    where
        R: ?Sized + NixRead + Send,
    {
        if let Some(tag) = reader.try_read_value::<FieldType>().await? {
            match tag {
                FieldType::Int => {
                    let v = reader.read_value().await?;
                    Ok(Some(Field::Int(v)))
                }
                FieldType::String => {
                    let s = reader.read_value().await?;
                    Ok(Some(Field::String(s)))
                }
            }
        } else {
            Ok(None)
        }
    }
}

// OutputName
nix_deserialize_remote!(
    #[nix(from_str)]
    harmonia_store_core::derived_path::OutputName
);
nix_serialize_remote!(
    #[nix(display)]
    harmonia_store_core::derived_path::OutputName
);

// Algorithm
nix_deserialize_remote!(
    #[nix(from_str)]
    harmonia_store_core::hash::Algorithm
);
nix_serialize_remote!(
    #[nix(display)]
    harmonia_store_core::hash::Algorithm
);

// NarHash (uses custom from/into with fmt types)
nix_deserialize_remote!(
    #[nix(
        from = "harmonia_store_core::hash::fmt::Bare<harmonia_store_core::hash::fmt::Any<harmonia_store_core::hash::NarHash>>"
    )]
    harmonia_store_core::hash::NarHash
);
nix_serialize_remote!(
    #[nix(
        into = "harmonia_store_core::hash::fmt::Bare<harmonia_store_core::hash::fmt::Base16<harmonia_store_core::hash::NarHash>>"
    )]
    harmonia_store_core::hash::NarHash
);

// Signature
nix_deserialize_remote!(
    #[nix(from_str)]
    harmonia_store_core::signature::Signature
);
nix_serialize_remote!(
    #[nix(display)]
    harmonia_store_core::signature::Signature
);

// ContentAddress
nix_deserialize_remote!(
    #[nix(from_str)]
    harmonia_store_core::store_path::ContentAddress
);
nix_serialize_remote!(
    #[nix(display)]
    harmonia_store_core::store_path::ContentAddress
);

// Hash and its format wrappers
nix_deserialize_remote!(#[nix(from_str)] harmonia_store_core::hash::fmt::Any<harmonia_store_core::hash::Hash>);
nix_serialize_remote!(#[nix(display)] harmonia_store_core::hash::fmt::Base32<harmonia_store_core::hash::Hash>);

nix_deserialize_remote!(
    #[nix(from = "harmonia_store_core::hash::fmt::Any<harmonia_store_core::hash::Hash>")]
    harmonia_store_core::hash::Hash
);
nix_serialize_remote!(
    #[nix(into = "harmonia_store_core::hash::fmt::Base32<harmonia_store_core::hash::Hash>")]
    harmonia_store_core::hash::Hash
);

// NarHash format wrappers
nix_deserialize_remote!(#[nix(from_str)] harmonia_store_core::hash::fmt::Bare<harmonia_store_core::hash::fmt::Any<harmonia_store_core::hash::NarHash>>);
nix_serialize_remote!(#[nix(display)] harmonia_store_core::hash::fmt::Bare<harmonia_store_core::hash::fmt::Base16<harmonia_store_core::hash::NarHash>>);

// ContentAddressMethodAlgorithm
nix_deserialize_remote!(
    #[nix(from_str)]
    harmonia_store_core::store_path::ContentAddressMethodAlgorithm
);
nix_serialize_remote!(
    #[nix(display)]
    harmonia_store_core::store_path::ContentAddressMethodAlgorithm
);

// StorePathHash
nix_deserialize_remote!(
    #[nix(from_str)]
    harmonia_store_core::store_path::StorePathHash
);
nix_serialize_remote!(
    #[nix(display)]
    harmonia_store_core::store_path::StorePathHash
);

// DrvOutput
nix_deserialize_remote!(
    #[nix(from_str)]
    harmonia_store_core::realisation::DrvOutput
);
nix_serialize_remote!(
    #[nix(display)]
    harmonia_store_core::realisation::DrvOutput
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::de::NixRead;
    use crate::ser::NixWrite;
    use harmonia_store_core::realisation::Realisation;
    use rstest::rstest;
    use std::collections::BTreeMap;

    macro_rules! set {
        () => { std::collections::BTreeSet::new() };
        ($($x:expr),+ $(,)?) => {{
            let mut ret = std::collections::BTreeSet::new();
            $(
                ret.insert($x.parse().unwrap());
            )+
            ret
        }};
    }

    macro_rules! btree_map {
        () => { BTreeMap::new() };
        ($($k:expr => $v:expr),+ $(,)?) => {{
            let mut ret = BTreeMap::new();
            $(
                ret.insert($k.parse().unwrap(), $v.parse().unwrap());
            )+
            ret
        }};
    }

    #[tokio::test]
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
    async fn nix_write_realisation(#[case] value: Realisation, #[case] expected: &str) {
        let mut mock = crate::ser::mock::Builder::new()
            .write_slice(expected.as_bytes())
            .build();
        mock.write_value(&value).await.unwrap();
    }

    #[tokio::test]
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
    async fn nix_read_realisation(#[case] expected: Realisation, #[case] value: &str) {
        let mut mock = crate::de::mock::Builder::new()
            .read_slice(value.as_bytes())
            .build();
        let actual: Realisation = mock.read_value().await.unwrap();
        pretty_assertions::assert_eq!(actual, expected);
    }
}
