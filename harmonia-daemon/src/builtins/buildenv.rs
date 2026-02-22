// SPDX-FileCopyrightText: 2026 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! Builtin: buildenv — create a symlink tree (user environment) from packages.
//!
//! Reads the package paths from the derivation env and creates a directory
//! at `$out` with symlinks into each package.

use std::collections::BTreeMap;
use std::path::PathBuf;

use harmonia_store_core::derivation::BasicDerivation;
use harmonia_store_core::derived_path::OutputName;
use harmonia_store_core::store_path::{StoreDir, StorePath};

use crate::build::BuildError;

pub(crate) async fn builtin_buildenv(
    _drv: &BasicDerivation,
    env: &BTreeMap<String, String>,
    output_paths: &[(OutputName, StorePath)],
    store_dir: &StoreDir,
) -> Result<(), BuildError> {
    let (_, out_path) = output_paths
        .first()
        .ok_or_else(|| BuildError::Other("buildenv: no output path".to_string()))?;
    let dest = store_dir.to_path().join(out_path.to_string());

    // Read the manifest (JSON array of package paths)
    let manifest = env.get("manifest").ok_or_else(|| {
        BuildError::Other("builtin:buildenv requires 'manifest' env var".to_string())
    })?;

    // Parse the manifest as a JSON array of objects with `paths` fields
    let manifest_json: serde_json::Value = serde_json::from_str(manifest)
        .map_err(|e| BuildError::Other(format!("builtin:buildenv: invalid manifest JSON: {e}")))?;

    std::fs::create_dir_all(&dest).map_err(|e| {
        BuildError::Other(format!(
            "builtin:buildenv: failed to create output dir: {e}"
        ))
    })?;

    // Simple implementation: create symlinks for each package's top-level entries
    if let Some(pkgs) = manifest_json.as_array() {
        for pkg in pkgs {
            if let Some(paths) = pkg.get("paths").and_then(|p| p.as_array()) {
                for path_val in paths {
                    if let Some(pkg_path) = path_val.as_str() {
                        let pkg_dir = PathBuf::from(pkg_path);
                        if pkg_dir.is_dir() {
                            // Symlink each entry in the package dir into the output
                            if let Ok(entries) = std::fs::read_dir(&pkg_dir) {
                                for entry in entries.flatten() {
                                    let name = entry.file_name();
                                    let link = dest.join(&name);
                                    if !link.exists() {
                                        let _ = std::os::unix::fs::symlink(entry.path(), &link);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use harmonia_protocol::build_result::BuildResultInner;
    use harmonia_protocol::daemon_wire::types2::BuildMode;
    use harmonia_store_core::derivation::{DerivationOutput, DerivationT};
    use harmonia_store_core::derived_path::OutputName;
    use harmonia_store_core::store_path::StorePath;

    use crate::tests::test_store::TestStore;

    /// `builtin:buildenv` with a list of packages → symlink tree created at output path.
    #[tokio::test]
    async fn test_builtin_buildenv() {
        let ts = TestStore::new();

        // Create a "package" directory with some files
        let pkg_dir = ts
            .store_path()
            .join("pppppppppppppppppppppppppppppppp-mypkg");
        std::fs::create_dir_all(pkg_dir.join("bin")).unwrap();
        std::fs::write(pkg_dir.join("bin/hello"), "#!/bin/sh\necho hi").unwrap();
        std::fs::create_dir_all(pkg_dir.join("lib")).unwrap();
        std::fs::write(pkg_dir.join("lib/libfoo.so"), "fake lib").unwrap();

        let output_path =
            StorePath::from_base_path("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-myenv").unwrap();
        let drv_path =
            StorePath::from_base_path("zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz-myenv.drv").unwrap();

        let mut outputs = BTreeMap::new();
        outputs.insert(
            OutputName::default(),
            DerivationOutput::InputAddressed(output_path.clone()),
        );

        // The manifest is a JSON array like nix-env uses
        let manifest = serde_json::json!([{
            "paths": [pkg_dir.to_string_lossy()]
        }]);

        let mut env = BTreeMap::new();
        env.insert("manifest".into(), manifest.to_string().into());

        let drv = DerivationT {
            name: "myenv".parse().unwrap(),
            outputs,
            inputs: BTreeSet::new(),
            platform: "x86_64-linux".into(),
            builder: "builtin:buildenv".into(),
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

        // The output should have symlinks to the package's top-level entries
        let out_disk = ts.store_path().join(output_path.to_string());
        assert!(out_disk.exists(), "Output dir should exist");
        assert!(out_disk.join("bin").exists(), "bin symlink should exist");
        assert!(out_disk.join("lib").exists(), "lib symlink should exist");
        // Verify they are symlinks
        assert!(
            out_disk
                .join("bin")
                .symlink_metadata()
                .unwrap()
                .file_type()
                .is_symlink(),
            "bin should be a symlink"
        );
    }
}
