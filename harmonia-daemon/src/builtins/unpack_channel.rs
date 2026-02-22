// SPDX-FileCopyrightText: 2026 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! Builtin: unpack-channel — unpack a channel tarball into the output path.
//!
//! Matches Nix's `builtinUnpackChannel` behavior:
//! 1. Read the `src` tarball and `channelName` from the derivation env.
//! 2. Validate that `channelName` contains no path separators.
//! 3. Extract the tarball into `$out` using libarchive (via `compress-tools`),
//!    which auto-detects all compression formats that libarchive supports
//!    (gzip, bzip2, xz, zstd, lz4, lzma, etc.) — matching Nix's use of
//!    `archive_read_support_filter_all`.
//! 4. Verify exactly one top-level entry was extracted.
//! 5. Rename that entry to `$out/$channelName`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use harmonia_store_core::derivation::BasicDerivation;
use harmonia_store_core::derived_path::OutputName;
use harmonia_store_core::store_path::{StoreDir, StorePath};

use crate::build::BuildError;

pub async fn builtin_unpack_channel(
    _drv: &BasicDerivation,
    env: &BTreeMap<String, String>,
    output_paths: &[(OutputName, StorePath)],
    store_dir: &StoreDir,
) -> Result<(), BuildError> {
    let (_, out_path) = output_paths
        .first()
        .ok_or_else(|| BuildError::Other("unpack-channel: no output path".to_string()))?;
    let dest = store_dir.to_path().join(out_path.to_string());

    let src = env.get("src").ok_or_else(|| {
        BuildError::Other("builtin:unpack-channel requires 'src' env var".to_string())
    })?;

    let channel_name = env.get("channelName").ok_or_else(|| {
        BuildError::Other("builtin:unpack-channel requires 'channelName' env var".to_string())
    })?;

    // Validate channelName contains no path separators (matches Nix's check)
    if Path::new(channel_name)
        .file_name()
        .map(|f| f != channel_name.as_str())
        .unwrap_or(true)
    {
        return Err(BuildError::Other(format!(
            "channelName is not allowed to contain filesystem separators, got {channel_name}"
        )));
    }

    std::fs::create_dir_all(&dest).map_err(|e| {
        BuildError::Other(format!(
            "builtin:unpack-channel: failed to create output dir: {e}"
        ))
    })?;

    // Extract the tarball using libarchive (via compress-tools), which
    // auto-detects all supported compression formats — matching Nix's
    // archive_read_support_filter_all + archive_read_support_format_tar/zip.
    let source = std::fs::File::open(src).map_err(|e| {
        BuildError::Other(format!(
            "builtin:unpack-channel: failed to open '{src}': {e}"
        ))
    })?;
    compress_tools::uncompress_archive(source, &dest, compress_tools::Ownership::Ignore)
        .map_err(|e| BuildError::Other(format!("builtin:unpack-channel: extraction error: {e}")))?;

    // Nix expects exactly one top-level entry in the tarball, then renames
    // it to $out/$channelName.
    let mut entries: Vec<PathBuf> = std::fs::read_dir(&dest)
        .map_err(|e| BuildError::Other(format!("builtin:unpack-channel: read_dir error: {e}")))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();

    if entries.len() != 1 {
        return Err(BuildError::Other(format!(
            "channel tarball '{}' contains {} entries, expected exactly 1",
            src,
            entries.len()
        )));
    }

    let extracted = entries.remove(0);
    let target = dest.join(channel_name);
    std::fs::rename(&extracted, &target).map_err(|e| {
        BuildError::Other(format!(
            "builtin:unpack-channel: failed to rename {} to {}: {e}",
            extracted.display(),
            target.display()
        ))
    })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use harmonia_protocol::build_result::BuildResultInner;
    use harmonia_protocol::daemon_wire::types2::BuildMode;
    use harmonia_store_core::derivation::{DerivationOutput, DerivationT};
    use harmonia_store_core::derived_path::OutputName;
    use harmonia_store_core::store_path::StorePath;
    use std::collections::{BTreeMap, BTreeSet};

    use crate::tests::test_store::TestStore;

    /// Create a compressed channel tarball at `path` using the `tar` CLI
    /// with the given compression flag (e.g. "--gzip", "--bzip2", "--xz").
    fn create_channel_tarball(path: &std::path::Path, tar_compress_flag: &str) {
        use std::process::Command;

        let dir = tempfile::tempdir().unwrap();
        let channel_dir = dir.path().join("nixos-24.05");
        std::fs::create_dir_all(&channel_dir).unwrap();
        std::fs::write(channel_dir.join("default.nix"), "channel content here").unwrap();

        let status = Command::new("tar")
            .arg("cf")
            .arg(path)
            .arg(tar_compress_flag)
            .arg("-C")
            .arg(dir.path())
            .arg("nixos-24.05")
            .status()
            .expect("tar command not found");
        assert!(status.success(), "tar failed");
    }

    /// Run the full unpack-channel builtin pipeline and verify the result.
    async fn run_unpack_channel_test(tar_compress_flag: &str) {
        let tarball_dir = tempfile::tempdir().unwrap();
        let tarball_path = tarball_dir.path().join("channel.tar.compressed");
        create_channel_tarball(&tarball_path, tar_compress_flag);

        let ts = TestStore::new();

        let output_path =
            StorePath::from_base_path("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-mychannel").unwrap();
        let drv_path =
            StorePath::from_base_path("zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz-mychannel.drv").unwrap();

        let mut outputs = BTreeMap::new();
        outputs.insert(
            OutputName::default(),
            DerivationOutput::InputAddressed(output_path.clone()),
        );

        let mut env = BTreeMap::new();
        env.insert(
            "src".into(),
            tarball_path.to_string_lossy().to_string().into(),
        );
        env.insert("channelName".into(), "mychannel".into());

        let drv = DerivationT {
            name: "mychannel".parse().unwrap(),
            outputs,
            inputs: BTreeSet::new(),
            platform: "x86_64-linux".into(),
            builder: "builtin:unpack-channel".into(),
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

        let out_disk = ts.store_path().join(output_path.to_string());
        // Nix extracts into $out then renames the single top-level dir to $channelName
        let content =
            std::fs::read_to_string(out_disk.join("mychannel").join("default.nix")).unwrap();
        assert_eq!(content, "channel content here");
    }

    #[tokio::test]
    async fn test_unpack_channel_gzip() {
        run_unpack_channel_test("--gzip").await;
    }

    #[tokio::test]
    async fn test_unpack_channel_bzip2() {
        run_unpack_channel_test("--bzip2").await;
    }

    #[tokio::test]
    async fn test_unpack_channel_xz() {
        run_unpack_channel_test("--xz").await;
    }
}
