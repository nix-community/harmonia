// SPDX-FileCopyrightText: 2026 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! Builtin: fetchurl — download a URL to the output path.
//!
//! Reads `url` from the derivation env and downloads it to `$out`.
//! For `unpack = 1`, the downloaded file is extracted as a NAR.

use std::collections::BTreeMap;

use harmonia_store_core::derivation::BasicDerivation;
use harmonia_store_core::derived_path::OutputName;
use harmonia_store_core::store_path::{StoreDir, StorePath};

use crate::build::BuildError;

pub(crate) async fn builtin_fetchurl(
    _drv: &BasicDerivation,
    env: &BTreeMap<String, String>,
    output_paths: &[(OutputName, StorePath)],
    store_dir: &StoreDir,
) -> Result<(), BuildError> {
    let url = env
        .get("url")
        .ok_or_else(|| BuildError::Other("builtin:fetchurl requires 'url' env var".to_string()))?;

    let (_, out_path) = output_paths
        .first()
        .ok_or_else(|| BuildError::Other("fetchurl: no output path".to_string()))?;
    let dest = store_dir.to_path().join(out_path.to_string());

    // Support file:// URLs by reading directly from disk
    if let Some(file_path) = url.strip_prefix("file://") {
        let content = tokio::fs::read(file_path).await.map_err(|e| {
            BuildError::Other(format!(
                "builtin:fetchurl failed to read '{file_path}': {e}"
            ))
        })?;

        let unpack = env.get("unpack").is_some_and(|v| v == "1");
        if unpack {
            // Treat the file as a NAR and restore it
            let cursor = std::io::Cursor::new(content);
            let events = harmonia_nar::parse_nar(cursor);
            use futures::StreamExt as _;
            let mapped = events.map(|item| match item {
                Ok(event) => Ok(event),
                Err(e) => Err(harmonia_nar::NarWriteError::create_file_error(
                    dest.clone(),
                    e,
                )),
            });
            harmonia_nar::restore(mapped, &dest)
                .await
                .map_err(|e| BuildError::Other(format!("builtin:fetchurl unpack error: {e}")))?;
        } else {
            tokio::fs::write(&dest, &content).await.map_err(|e| {
                BuildError::Other(format!(
                    "builtin:fetchurl failed to write '{}': {e}",
                    dest.display()
                ))
            })?;
        }
        Ok(())
    } else {
        Err(BuildError::Other(format!(
            "builtin:fetchurl: only file:// URLs are supported (got '{url}')"
        )))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use harmonia_protocol::build_result::BuildResultInner;
    use harmonia_protocol::daemon_wire::types2::BuildMode;
    use harmonia_store_core::derivation::{DerivationOutput, DerivationT};
    use harmonia_store_core::derived_path::OutputName;
    use harmonia_store_core::store_path::{ContentAddress, StorePath};

    use crate::tests::test_store::TestStore;

    /// `builtin:fetchurl` with a file:// URL → output contains downloaded content.
    #[tokio::test]
    async fn test_builtin_fetchurl() {
        let ts = TestStore::new();

        // Create a source file to "download"
        let source_dir = tempfile::tempdir().unwrap();
        let source_file = source_dir.path().join("content.txt");
        std::fs::write(&source_file, "fetched content here").unwrap();
        let file_url = format!("file://{}", source_file.display());

        // Use CAFixed for fetchurl (it's a fixed-output derivation)
        let ca: ContentAddress =
            "fixed:sha256:248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
                .parse()
                .unwrap();
        let output = DerivationOutput::CAFixed(ca);

        let output_path = output
            .path(
                &ts.store_dir,
                &"fetchurl-test".parse().unwrap(),
                &OutputName::default(),
            )
            .unwrap()
            .expect("CAFixed should produce a known output path");

        let drv_path =
            StorePath::from_base_path("zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz-fetchurl-test.drv")
                .unwrap();

        let mut outputs = BTreeMap::new();
        outputs.insert(OutputName::default(), output);

        let mut env = BTreeMap::new();
        env.insert("url".into(), file_url.into());

        let drv = DerivationT {
            name: "fetchurl-test".parse().unwrap(),
            outputs,
            inputs: BTreeSet::new(),
            platform: "x86_64-linux".into(),
            builder: "builtin:fetchurl".into(),
            args: vec![],
            env,
            structured_attrs: None,
        };

        let config = crate::build::BuildConfig {
            build_dir: ts.build_dir(),
            log_dir: None,
            ..Default::default()
        };

        let result = crate::build::build_derivation(
            &ts.store_dir,
            &ts.db,
            &drv_path,
            &drv,
            BuildMode::Normal,
            &config,
            &crate::config::SandboxConfig::Off,
        )
        .await
        .unwrap();

        match &result.inner {
            BuildResultInner::Failure(f) => {
                panic!(
                    "Expected success, got failure: {:?} - {}",
                    f.status,
                    String::from_utf8_lossy(&f.error_msg)
                );
            }
            BuildResultInner::Success(_) => {}
        }

        // Verify the output contains the "downloaded" content
        let disk_path = ts.store_path().join(output_path.to_string());
        let content = std::fs::read_to_string(&disk_path).unwrap();
        assert_eq!(content, "fetched content here");
    }
}
