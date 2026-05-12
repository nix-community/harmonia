//! Shared helpers and macros for reading/writing wire protocol test fixtures.

use std::io::Cursor;
use std::path::PathBuf;

use tokio::io::AsyncWriteExt as _;

use harmonia_protocol::de::{NixDeserialize, NixRead, NixReader};
use harmonia_protocol::ser::{NixSerialize, NixWrite, NixWriter};
use harmonia_protocol::version::FeatureSet;
use harmonia_utils_test::json_upstream::{read_upstream_json, upstream_test_data_path};

pub fn worker_protocol_data_path(stem: &str) -> PathBuf {
    upstream_test_data_path()
        .join("libstore-tests/data/worker-protocol")
        .join(stem)
}

pub fn read_upstream_bin(stem: &str) -> Vec<u8> {
    let path = worker_protocol_data_path(&format!("{stem}.bin"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("Failed to read {}: {}", path.display(), e))
}

pub fn read_upstream_wire_json<T>(stem: &str) -> T
where
    T: for<'de> serde::Deserialize<'de>,
{
    let path = worker_protocol_data_path(&format!("{stem}.json"));
    read_upstream_json(&path)
}

/// Deserialize a sequence of values from wire bytes (tuple-style, no length
/// prefix — read elements until EOF).
pub async fn read_many_from_wire<T: NixDeserialize + Send>(
    features: FeatureSet,
    bin: &[u8],
) -> Vec<T> {
    let mut reader = NixReader::builder()
        .set_features(features)
        .build_buffered(Cursor::new(bin.to_vec()));
    let mut results = Vec::new();
    while let Some(v) = reader.try_read_value::<T>().await.unwrap() {
        results.push(v);
    }
    results
}

/// Serialize a sequence of values to wire bytes (tuple-style, no length prefix).
pub async fn write_many_to_wire<T: NixSerialize + Send + Sync>(
    features: FeatureSet,
    values: &[T],
) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut writer = NixWriter::builder().set_features(features).build(&mut buf);
    for v in values {
        writer.write_value(v).await.unwrap();
    }
    writer.flush().await.unwrap();
    drop(writer);
    buf
}

/// Deserialize a single value from wire bytes.
pub async fn read_one_from_wire<T: NixDeserialize + Send>(features: FeatureSet, bin: &[u8]) -> T {
    let mut reader = NixReader::builder()
        .set_features(features)
        .build_buffered(Cursor::new(bin.to_vec()));
    reader.read_value().await.unwrap()
}

/// Serialize a single value to wire bytes.
pub async fn write_one_to_wire<T: NixSerialize + Send + Sync>(
    features: FeatureSet,
    value: &T,
) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut writer = NixWriter::builder().set_features(features).build(&mut buf);
    writer.write_value(value).await.unwrap();
    writer.flush().await.unwrap();
    drop(writer);
    buf
}

// ---------------------------------------------------------------------------
// Test-generating macros
// ---------------------------------------------------------------------------

/// Generate read + write tests for a fixture containing a homogeneous
/// sequence of values (upstream `std::tuple<T, T, …>`).
macro_rules! test_upstream_wire {
    ($test_name:ident, $stem:expr, $ty:ty, $features:expr) => {
        mod $test_name {
            #[allow(unused_imports)]
            use super::*;

            #[tokio::test]
            async fn from_wire() {
                let expected: Vec<$ty> = crate::test_infra::read_upstream_wire_json($stem);
                let bin = crate::test_infra::read_upstream_bin($stem);
                let parsed: Vec<$ty> =
                    crate::test_infra::read_many_from_wire($features, &bin).await;
                assert_eq!(parsed, expected);
            }

            #[tokio::test]
            async fn to_wire() {
                let values: Vec<$ty> = crate::test_infra::read_upstream_wire_json($stem);
                let expected_bin = crate::test_infra::read_upstream_bin($stem);
                let written = crate::test_infra::write_many_to_wire($features, &values).await;
                assert_eq!(written, expected_bin);
            }
        }
    };
}

/// Like [`test_upstream_wire!`] but only generates the `from_wire` read test.
macro_rules! test_upstream_wire_read {
    ($test_name:ident, $stem:expr, $ty:ty, $features:expr) => {
        mod $test_name {
            #[allow(unused_imports)]
            use super::*;

            #[tokio::test]
            async fn from_wire() {
                let expected: Vec<$ty> = crate::test_infra::read_upstream_wire_json($stem);
                let bin = crate::test_infra::read_upstream_bin($stem);
                let parsed: Vec<$ty> =
                    crate::test_infra::read_many_from_wire($features, &bin).await;
                assert_eq!(parsed, expected);
            }
        }
    };
}

/// Generate read + write tests for a fixture containing a single value.
macro_rules! test_upstream_wire_single {
    ($test_name:ident, $stem:expr, $ty:ty, $features:expr) => {
        mod $test_name {
            #[allow(unused_imports)]
            use super::*;

            #[tokio::test]
            async fn from_wire() {
                let expected: $ty = crate::test_infra::read_upstream_wire_json($stem);
                let bin = crate::test_infra::read_upstream_bin($stem);
                let parsed: $ty = crate::test_infra::read_one_from_wire($features, &bin).await;
                assert_eq!(parsed, expected);
            }

            #[tokio::test]
            async fn to_wire() {
                let value: $ty = crate::test_infra::read_upstream_wire_json($stem);
                let expected_bin = crate::test_infra::read_upstream_bin($stem);
                let written = crate::test_infra::write_one_to_wire($features, &value).await;
                assert_eq!(written, expected_bin);
            }
        }
    };
}

pub(crate) use test_upstream_wire;
pub(crate) use test_upstream_wire_read;
pub(crate) use test_upstream_wire_single;
