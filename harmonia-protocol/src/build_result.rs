//! `NixDeserialize`/`NixSerialize` impls for `BuildResult` and related types.
//!
//! The pure types and JSON serialization live in `harmonia-store-build-result`.

use std::collections::BTreeMap;

use bytes::Bytes;

use harmonia_store_build_result::{
    BuildResult, BuildResultFailure, BuildResultInner, BuildResultSuccess, FailureStatus,
    Microseconds, SuccessStatus,
};

use crate::de::{Error as _, NixDeserialize as NixDeserializeTrait, NixRead};
use crate::ser::{NixSerialize as NixSerializeTrait, NixWrite};
use harmonia_store_core::derived_path::OutputName;
use harmonia_store_core::realisation::UnkeyedRealisation;

// Wire protocol serialization - maintains compatibility with the flat status enum format

impl NixDeserializeTrait for Microseconds {
    async fn try_deserialize<R>(reader: &mut R) -> Result<Option<Self>, R::Error>
    where
        R: ?Sized + NixRead + Send,
    {
        Ok(reader
            .try_read_value::<i64>()
            .await?
            .map(Microseconds::from))
    }
}

impl NixSerializeTrait for Microseconds {
    async fn serialize<W>(&self, writer: &mut W) -> Result<(), W::Error>
    where
        W: NixWrite,
    {
        let v: i64 = (*self).into();
        writer.write_value(&v).await
    }
}

impl NixDeserializeTrait for BuildResult {
    async fn try_deserialize<R>(reader: &mut R) -> Result<Option<Self>, R::Error>
    where
        R: ?Sized + NixRead + Send,
    {
        let Some(status_raw) = reader.try_read_value::<u16>().await? else {
            return Ok(None);
        };
        let error_msg: Vec<u8> = reader.read_value::<Bytes>().await?.to_vec();
        let times_built: u32 = reader.read_value().await?;
        let is_non_deterministic: bool = reader.read_value().await?;
        let start_time: i64 = reader.read_value().await?;
        let stop_time: i64 = reader.read_value().await?;
        let cpu_user: Option<Microseconds> = reader.read_value().await?;
        let cpu_system: Option<Microseconds> = reader.read_value().await?;
        let built_outputs: BTreeMap<OutputName, UnkeyedRealisation> =
            if reader.has_feature(crate::version::FEATURE_REALISATION_WITH_PATH) {
                reader.read_value().await?
            } else {
                // Legacy peers send a StringMap of JSON realisations. We don't
                // implement the back-compat parsing; just drain the map and
                // discard, since harmonia never builds.
                let _ignored: BTreeMap<String, String> = reader.read_value().await?;
                BTreeMap::new()
            };

        let inner = if let Ok(status) = SuccessStatus::try_from(status_raw) {
            BuildResultInner::Success(BuildResultSuccess {
                status,
                built_outputs,
            })
        } else if let Ok(status) = FailureStatus::try_from(status_raw) {
            BuildResultInner::Failure(BuildResultFailure {
                status,
                error_msg,
                is_non_deterministic,
            })
        } else {
            return Err(R::Error::invalid_data(format!(
                "invalid build status: {}",
                status_raw
            )));
        };

        Ok(Some(BuildResult {
            inner,
            times_built,
            start_time,
            stop_time,
            cpu_user,
            cpu_system,
        }))
    }
}

impl NixSerializeTrait for BuildResult {
    async fn serialize<W>(&self, writer: &mut W) -> Result<(), W::Error>
    where
        W: NixWrite,
    {
        let (status_raw, error_msg, is_non_deterministic, built_outputs): (
            u16,
            &[u8],
            bool,
            &BTreeMap<OutputName, UnkeyedRealisation>,
        ) = match &self.inner {
            BuildResultInner::Success(s) => (s.status.into(), &[], false, &s.built_outputs),
            BuildResultInner::Failure(f) => {
                static EMPTY_MAP: BTreeMap<OutputName, UnkeyedRealisation> = BTreeMap::new();
                (
                    f.status.into(),
                    f.error_msg.as_slice(),
                    f.is_non_deterministic,
                    &EMPTY_MAP,
                )
            }
        };

        writer.write_value(&status_raw).await?;
        writer.write_slice(error_msg).await?;
        writer.write_value(&self.times_built).await?;
        writer.write_value(&is_non_deterministic).await?;
        writer.write_value(&self.start_time).await?;
        writer.write_value(&self.stop_time).await?;
        writer.write_value(&self.cpu_user).await?;
        writer.write_value(&self.cpu_system).await?;
        if writer.has_feature(crate::version::FEATURE_REALISATION_WITH_PATH) {
            writer.write_value(built_outputs).await?;
        } else {
            // Legacy peers expect a StringMap of JSON realisations keyed by
            // `sha256:<hex>!<out>`. The hash modulo no longer exists; old
            // clients only extract `outputName` and `outPath`, so a dummy hash
            // suffices.
            let dummy_hash =
                "sha256:0000000000000000000000000000000000000000000000000000000000000000";
            writer.write_value(&built_outputs.len()).await?;
            for (output_name, realisation) in built_outputs {
                let id = format!("{dummy_hash}!{output_name}");
                let json = format!(r#"{{"id":"{id}","outPath":"{}"}}"#, realisation.out_path);
                writer.write_slice(id.as_bytes()).await?;
                writer.write_slice(json.as_bytes()).await?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use tokio::io::AsyncWriteExt as _;

    use super::*;
    use crate::de::{NixRead, NixReader};
    use crate::ser::{NixWrite, NixWriter};

    fn sample_success() -> BuildResult {
        BuildResult {
            inner: BuildResultInner::Success(BuildResultSuccess {
                status: SuccessStatus::Built,
                built_outputs: [(
                    "out".parse().unwrap(),
                    UnkeyedRealisation {
                        out_path: "g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-foo".parse().unwrap(),
                        signatures: Default::default(),
                    },
                )]
                .into(),
            }),
            times_built: 1,
            start_time: 30,
            stop_time: 50,
            cpu_user: Some(500.into()),
            cpu_system: Some(604.into()),
        }
    }

    async fn write(features: crate::version::FeatureSet, v: &BuildResult) -> Vec<u8> {
        let mut buf = Vec::new();
        let mut w = NixWriter::builder().set_features(features).build(&mut buf);
        w.write_value(v).await.unwrap();
        w.flush().await.unwrap();
        drop(w);
        buf
    }

    async fn read(features: crate::version::FeatureSet, buf: Vec<u8>) -> BuildResult {
        let mut r = NixReader::builder()
            .set_features(features)
            .build_buffered(Cursor::new(buf));
        r.read_value().await.unwrap()
    }

    /// Without the feature, the writer must emit the legacy StringMap form
    /// (so an old peer keeps decoding), and our reader must be able to drain
    /// it without desyncing the stream — built_outputs are simply dropped.
    #[tokio::test]
    async fn wire_roundtrip_without_feature_degrades_gracefully() {
        let v = sample_success();
        let buf = write(Default::default(), &v).await;
        assert!(buf.windows(4).any(|w| w == b"!out"));
        let back = read(Default::default(), buf).await;
        let BuildResultInner::Success(s) = &back.inner else {
            panic!("expected success")
        };
        assert!(s.built_outputs.is_empty());
        assert_eq!(back.times_built, v.times_built);
        assert_eq!(back.start_time, v.start_time);
    }
}
