//! Worker protocol wire tests for `ValidPathInfo`.
//!
//! The wire format has been stable since protocol 1.16, so we test the
//! `valid-path-info-1.16` fixture even though 1.16 is below our minimum
//! version number.
//!
//! `ValidPathInfo` (`StorePathKeyed<UnkeyedValidPathInfo>`) doesn't impl
//! serde `Deserialize`, so we parse the JSON manually.

use harmonia_store_core::store_path::StorePath;
use harmonia_store_path_info::{UnkeyedValidPathInfo, ValidPathInfo};

use crate::no_features;

/// Parse the upstream JSON fixture into `Vec<ValidPathInfo>`.
///
/// Each JSON object has a `"path"` field alongside the `UnkeyedValidPathInfo`
/// fields. We extract `path` then deserialize the rest.
fn parse_valid_path_info_json(stem: &str) -> Vec<ValidPathInfo> {
    let values: Vec<serde_json::Value> = crate::test_infra::read_upstream_wire_json(stem);
    values
        .into_iter()
        .map(|mut obj| {
            let path: StorePath =
                serde_json::from_value(obj.as_object_mut().unwrap().remove("path").unwrap())
                    .unwrap();
            // Re-serialize to string so that NarHash can deserialize from
            // a borrowed &str (serde_json::from_value only provides owned strings).
            let json_str = serde_json::to_string(&obj).unwrap();
            let info: UnkeyedValidPathInfo = serde_json::from_str(&json_str).unwrap();
            ValidPathInfo { path, info }
        })
        .collect()
}

mod valid_path_info_1_16 {
    use super::*;

    #[tokio::test]
    async fn from_wire() {
        let expected = parse_valid_path_info_json("valid-path-info-1.16");
        let bin = crate::test_infra::read_upstream_bin("valid-path-info-1.16");
        let parsed: Vec<ValidPathInfo> =
            crate::test_infra::read_many_from_wire(no_features(), &bin).await;
        assert_eq!(parsed, expected);
    }

    #[tokio::test]
    async fn to_wire() {
        let values = parse_valid_path_info_json("valid-path-info-1.16");
        let expected_bin = crate::test_infra::read_upstream_bin("valid-path-info-1.16");
        let written = crate::test_infra::write_many_to_wire(no_features(), &values).await;
        assert_eq!(written, expected_bin);
    }
}
