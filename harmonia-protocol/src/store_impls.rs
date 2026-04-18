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

use std::collections::BTreeMap;
use std::str::FromStr;

use bytes::Bytes;

use crate::daemon_wire::logger::RawLogMessageType;
use crate::de::{NixDeserialize, NixRead};
use crate::log::{Activity, ActivityResult, LogMessage, StopActivity};
use crate::ser::{NixSerialize, NixWrite};
use harmonia_protocol_derive::{nix_deserialize_remote, nix_serialize_remote};
use harmonia_store_core::derivation::{BasicDerivation, DerivationOutput, StructuredAttrs};
use harmonia_store_core::derived_path::{DerivedPath, LegacyDerivedPath, OutputName};
use harmonia_store_core::realisation::{DrvOutput, Realisation, UnkeyedRealisation};
use harmonia_store_core::store_path::{
    ContentAddress, ContentAddressMethodAlgorithm, StorePath, StorePathName,
};
use harmonia_utils_hash::fmt::CommonHash;

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
        // The wire format (like ATerm) carries structured attrs as the
        // `__json` env var, not as a separate field. Re-inject it so the
        // remote daemon reconstructs the same derivation.
        match &drv.structured_attrs {
            None => writer.write_value(&drv.env).await?,
            Some(sa) => {
                use crate::ser::Error as _;
                let mut env = drv.env.clone();
                let json = serde_json::to_string(&sa.attrs).map_err(|e| {
                    W::Error::custom(std::format_args!("failed to encode structured attrs: {e}"))
                })?;
                env.insert(Bytes::from_static(b"__json"), Bytes::from(json));
                writer.write_value(&env).await?;
            }
        }
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
            let mut env: BTreeMap<Bytes, Bytes> = reader.read_value().await?;
            // Mirror `StructuredAttrs::tryExtract`: pull `__json` out
            // of env into the dedicated field so round-tripping is lossless.
            let structured_attrs = env.remove(b"__json".as_slice()).and_then(|json_bytes| {
                let json_str = std::str::from_utf8(&json_bytes).ok()?;
                let attrs: serde_json::Map<String, serde_json::Value> =
                    serde_json::from_str(json_str).ok()?;
                Some(StructuredAttrs { attrs })
            });

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
                    structured_attrs,
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
    use harmonia_utils_hash::fmt::Base32;

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

// ========== Realisation / DrvOutput ==========
//
// Wire format requires the `realisation-with-path-not-hash` feature.
// Harmonia only advertises/accepts the new format; if the peer did not
// negotiate the feature these serializers fail explicitly before touching the
// stream.

fn require_realisation_feature_ser<W: NixWrite>(writer: &W) -> Result<(), W::Error> {
    use crate::ser::Error;
    if writer.has_feature(crate::version::FEATURE_REALISATION_WITH_PATH) {
        Ok(())
    } else {
        Err(W::Error::unsupported_data(format_args!(
            "peer is missing the '{}' protocol feature, needed to support content-addressing derivations",
            crate::version::FEATURE_REALISATION_WITH_PATH
        )))
    }
}

fn require_realisation_feature_de<R: ?Sized + NixRead>(reader: &R) -> Result<(), R::Error> {
    use crate::de::Error;
    if reader.has_feature(crate::version::FEATURE_REALISATION_WITH_PATH) {
        Ok(())
    } else {
        Err(R::Error::invalid_data(format_args!(
            "peer is missing the '{}' protocol feature, needed to support content-addressing derivations",
            crate::version::FEATURE_REALISATION_WITH_PATH
        )))
    }
}

impl NixSerialize for DrvOutput {
    async fn serialize<W>(&self, writer: &mut W) -> Result<(), W::Error>
    where
        W: NixWrite,
    {
        require_realisation_feature_ser(writer)?;
        writer.write_value(&self.drv_path).await?;
        writer.write_value(&self.output_name).await
    }
}

impl NixDeserialize for DrvOutput {
    async fn try_deserialize<R>(reader: &mut R) -> Result<Option<Self>, R::Error>
    where
        R: ?Sized + NixRead + Send,
    {
        require_realisation_feature_de(reader)?;
        let Some(drv_path) = reader.try_read_value::<StorePath>().await? else {
            return Ok(None);
        };
        let output_name = reader.read_value().await?;
        Ok(Some(DrvOutput {
            drv_path,
            output_name,
        }))
    }
}

impl NixSerialize for UnkeyedRealisation {
    async fn serialize<W>(&self, writer: &mut W) -> Result<(), W::Error>
    where
        W: NixWrite,
    {
        require_realisation_feature_ser(writer)?;
        writer.write_value(&self.out_path).await?;
        writer.write_value(&self.signatures).await
    }
}

impl NixDeserialize for UnkeyedRealisation {
    async fn try_deserialize<R>(reader: &mut R) -> Result<Option<Self>, R::Error>
    where
        R: ?Sized + NixRead + Send,
    {
        require_realisation_feature_de(reader)?;
        let Some(out_path) = reader.try_read_value::<StorePath>().await? else {
            return Ok(None);
        };
        let signatures = reader.read_value().await?;
        Ok(Some(UnkeyedRealisation {
            out_path,
            signatures,
        }))
    }
}

impl NixSerialize for Realisation {
    async fn serialize<W>(&self, writer: &mut W) -> Result<(), W::Error>
    where
        W: NixWrite,
    {
        writer.write_value(&self.id).await?;
        writer.write_value(&self.value).await
    }
}

impl NixDeserialize for Realisation {
    async fn try_deserialize<R>(reader: &mut R) -> Result<Option<Self>, R::Error>
    where
        R: ?Sized + NixRead + Send,
    {
        let Some(id) = reader.try_read_value::<DrvOutput>().await? else {
            return Ok(None);
        };
        let value = reader.read_value().await?;
        Ok(Some(Realisation { id, value }))
    }
}

/// `Option<UnkeyedRealisation>` uses a 0/1 tag word like upstream's
/// `WorkerProto::Serialise<std::optional<UnkeyedRealisation>>`.
impl NixSerialize for Option<UnkeyedRealisation> {
    async fn serialize<W>(&self, writer: &mut W) -> Result<(), W::Error>
    where
        W: NixWrite,
    {
        match self {
            None => writer.write_number(0).await,
            Some(v) => {
                writer.write_number(1).await?;
                writer.write_value(v).await
            }
        }
    }
}

impl NixDeserialize for Option<UnkeyedRealisation> {
    async fn try_deserialize<R>(reader: &mut R) -> Result<Option<Self>, R::Error>
    where
        R: ?Sized + NixRead + Send,
    {
        use crate::de::Error;
        let Some(tag) = reader.try_read_number().await? else {
            return Ok(None);
        };
        match tag {
            0 => Ok(Some(None)),
            1 => Ok(Some(Some(reader.read_value().await?))),
            _ => Err(R::Error::invalid_data(
                "invalid optional build trace from remote",
            )),
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
    crate::log::Verbosity
);
nix_serialize_remote!(
    #[nix(into = "u16")]
    crate::log::Verbosity
);

// ActivityType
nix_deserialize_remote!(
    #[nix(try_from = "u16")]
    crate::log::ActivityType
);
nix_serialize_remote!(
    #[nix(into = "u16")]
    crate::log::ActivityType
);

// ResultType
nix_deserialize_remote!(
    #[nix(try_from = "u16")]
    crate::log::ResultType
);
nix_serialize_remote!(
    #[nix(into = "u16")]
    crate::log::ResultType
);

// FieldType
nix_deserialize_remote!(
    #[nix(try_from = "u16")]
    crate::log::FieldType
);
nix_serialize_remote!(
    #[nix(into = "u16")]
    crate::log::FieldType
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
use crate::log::{Field, FieldType};

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
    harmonia_utils_hash::Algorithm
);
nix_serialize_remote!(
    #[nix(display)]
    harmonia_utils_hash::Algorithm
);

// NarHash (uses custom from/into with fmt types)
nix_deserialize_remote!(
    #[nix(from = "harmonia_utils_hash::fmt::Bare<harmonia_utils_hash::fmt::Any<crate::NarHash>>")]
    crate::NarHash
);
nix_serialize_remote!(
    #[nix(
        into = "harmonia_utils_hash::fmt::Bare<harmonia_utils_hash::fmt::Base16<crate::NarHash>>"
    )]
    crate::NarHash
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
nix_deserialize_remote!(#[nix(from_str)] harmonia_utils_hash::fmt::Any<harmonia_utils_hash::Hash>);
nix_serialize_remote!(#[nix(display)] harmonia_utils_hash::fmt::Base32<harmonia_utils_hash::Hash>);

nix_deserialize_remote!(
    #[nix(from = "harmonia_utils_hash::fmt::Any<harmonia_utils_hash::Hash>")]
    harmonia_utils_hash::Hash
);
nix_serialize_remote!(
    #[nix(into = "harmonia_utils_hash::fmt::Base32<harmonia_utils_hash::Hash>")]
    harmonia_utils_hash::Hash
);

// NarHash format wrappers
nix_deserialize_remote!(#[nix(from_str)] harmonia_utils_hash::fmt::Bare<harmonia_utils_hash::fmt::Any<crate::NarHash>>);
nix_serialize_remote!(#[nix(display)] harmonia_utils_hash::fmt::Bare<harmonia_utils_hash::fmt::Base16<crate::NarHash>>);

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

// StoreDir - uses the store_dir from the NixRead/NixWrite context
impl crate::de::NixDeserialize for harmonia_store_core::store_path::StoreDir {
    async fn try_deserialize<R>(reader: &mut R) -> Result<Option<Self>, R::Error>
    where
        R: ?Sized + crate::de::NixRead + Send,
    {
        Ok(Some(reader.store_dir().clone()))
    }
}

impl crate::ser::NixSerialize for harmonia_store_core::store_path::StoreDir {
    async fn serialize<W: crate::ser::NixWrite>(&self, _writer: &mut W) -> Result<(), W::Error> {
        // StoreDir is never serialized over the wire - it's a local configuration value
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::ser::NixWrite;
    use harmonia_store_core::realisation::{DrvOutput, Realisation, UnkeyedRealisation};

    fn sample_realisation() -> Realisation {
        Realisation {
            id: DrvOutput {
                drv_path: "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-bar.drv".parse().unwrap(),
                output_name: "out".parse().unwrap(),
            },
            value: UnkeyedRealisation {
                out_path: "7h7qgvs4kgzsn8a6rb273saxyqh4jxlz-konsole-18.12.3"
                    .parse()
                    .unwrap(),
                signatures: ["cache.nixos.org-1:0CpHca+06TwFp9VkMyz5OaphT3E8mnS+1SWymYlvFaghKSYPCMQ66TS1XPAr1+y9rfQZPLaHrBjjnIRktE/nAA==".parse().unwrap()].into(),
            },
        }
    }

    /// Pins the exact wire bytes for interop with upstream Nix; the
    /// `wire_roundtrip` proptests only check self-consistency.
    #[tokio::test]
    async fn nix_write_realisation() {
        let value = sample_realisation();
        let mut mock = crate::ser::mock::Builder::new()
            .write_display("/nix/store/g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-bar.drv")
            .write_display("out")
            .write_display("/nix/store/7h7qgvs4kgzsn8a6rb273saxyqh4jxlz-konsole-18.12.3")
            .write_number(1)
            .write_display("cache.nixos.org-1:0CpHca+06TwFp9VkMyz5OaphT3E8mnS+1SWymYlvFaghKSYPCMQ66TS1XPAr1+y9rfQZPLaHrBjjnIRktE/nAA==")
            .build();
        mock.write_value(&value).await.unwrap();
    }

    #[tokio::test]
    async fn nix_write_drv_output_missing_feature() {
        let value = sample_realisation().id;
        let mut mock = crate::ser::mock::Builder::new()
            .features(Default::default())
            .build();
        let err = mock.write_value(&value).await.unwrap_err();
        assert!(
            err.to_string()
                .contains(crate::version::FEATURE_REALISATION_WITH_PATH)
        );
    }
}
