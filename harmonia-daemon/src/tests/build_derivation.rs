// SPDX-FileCopyrightText: 2026 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! Tests for the `build_derivation` handler method.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use harmonia_protocol::build_result::{BuildResultInner, FailureStatus, SuccessStatus};
use harmonia_protocol::daemon::{DaemonStore, HandshakeDaemonStore};
use harmonia_protocol::daemon_wire::types2::BuildMode;
use harmonia_store_core::derivation::{
    BasicDerivation, DerivationOutput, DerivationT, StructuredAttrs,
};
use harmonia_store_core::derived_path::OutputName;
use harmonia_store_core::store_path::StorePath;

use super::test_store::TestStore;

/// Helper: create a multi-output derivation with `out` and `dev` outputs.
fn multi_output_derivation(_ts: &TestStore, name: &str) -> (StorePath, BasicDerivation) {
    let out_path =
        StorePath::from_base_path(&format!("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-{name}")).unwrap();
    let dev_path =
        StorePath::from_base_path(&format!("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-{name}-dev")).unwrap();
    let drv_path =
        StorePath::from_base_path(&format!("zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz-{name}.drv")).unwrap();

    let mut outputs = BTreeMap::new();
    outputs.insert(
        OutputName::default(), // "out"
        DerivationOutput::InputAddressed(out_path),
    );
    outputs.insert(
        "dev".parse().unwrap(),
        DerivationOutput::InputAddressed(dev_path),
    );

    let drv = DerivationT {
        name: name.parse().unwrap(),
        outputs,
        inputs: BTreeSet::new(),
        platform: "x86_64-linux".into(),
        builder: "/bin/sh".into(),
        args: vec![
            "-c".into(),
            "echo main > $out; echo dev-content > $dev".into(),
        ],
        env: BTreeMap::new(),
        structured_attrs: None,
    };

    (drv_path, drv)
}

/// Helper: create a simple single-output derivation that runs
/// `/bin/sh -c "echo hello > $out"`.
fn simple_echo_derivation(_ts: &TestStore, name: &str) -> (StorePath, BasicDerivation) {
    let output_path =
        StorePath::from_base_path(&format!("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-{name}")).unwrap();
    let drv_path =
        StorePath::from_base_path(&format!("zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz-{name}.drv")).unwrap();

    let mut outputs = BTreeMap::new();
    outputs.insert(
        OutputName::default(),
        DerivationOutput::InputAddressed(output_path),
    );

    let drv = DerivationT {
        name: name.parse().unwrap(),
        outputs,
        inputs: BTreeSet::new(),
        platform: "x86_64-linux".into(),
        builder: "/bin/sh".into(),
        args: vec!["-c".into(), "echo hello > $out".into()],
        env: BTreeMap::new(),
        structured_attrs: None,
    };

    (drv_path, drv)
}

/// Single-output build: `/bin/sh -c "echo hello > $out"` produces output
/// registered in DB with correct NAR hash, `BuildResult` status is `Built`.
#[tokio::test]
async fn test_build_derivation_single_output() {
    let ts = TestStore::new();
    let mut store = ts.handler.clone().handshake().await.unwrap();

    let (drv_path, drv) = simple_echo_derivation(&ts, "hello");

    let result = store
        .build_derivation(&drv_path, &drv, BuildMode::Normal)
        .await
        .unwrap();

    // Should succeed with Built status
    match &result.inner {
        BuildResultInner::Success(s) => {
            assert_eq!(s.status, SuccessStatus::Built);
        }
        BuildResultInner::Failure(f) => {
            panic!(
                "Expected success, got failure: {:?} - {}",
                f.status,
                String::from_utf8_lossy(&f.error_msg)
            );
        }
    }
    assert_eq!(result.times_built, 1);

    // Output should exist on disk
    let output_path = StorePath::from_base_path("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-hello").unwrap();
    let disk_path = ts.store_path().join(output_path.to_string());
    assert!(disk_path.exists(), "Output should exist on disk");

    // Read content
    let content = std::fs::read_to_string(&disk_path).unwrap();
    assert_eq!(content.trim(), "hello");

    // Output should be registered in the database
    let is_valid = store.is_valid_path(&output_path).await.unwrap();
    assert!(is_valid, "Output should be registered in DB");

    // Query path info and verify NAR hash is set
    let info = store.query_path_info(&output_path).await.unwrap();
    let info = info.expect("Should have path info in DB");
    assert!(info.nar_size > 0, "NAR size should be > 0");
}

/// Multi-output derivation (`out`, `dev`) → all outputs registered
/// with correct hashes and references.
#[tokio::test]
async fn test_build_derivation_multi_output() {
    let ts = TestStore::new();
    let mut store = ts.handler.clone().handshake().await.unwrap();

    let (drv_path, drv) = multi_output_derivation(&ts, "multi");

    let result = store
        .build_derivation(&drv_path, &drv, BuildMode::Normal)
        .await
        .unwrap();

    match &result.inner {
        BuildResultInner::Success(s) => {
            assert_eq!(s.status, SuccessStatus::Built);
        }
        BuildResultInner::Failure(f) => {
            panic!(
                "Expected success, got failure: {:?} - {}",
                f.status,
                String::from_utf8_lossy(&f.error_msg)
            );
        }
    }

    // Both outputs should exist on disk
    let out_path = StorePath::from_base_path("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-multi").unwrap();
    let dev_path = StorePath::from_base_path("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-multi-dev").unwrap();

    let out_disk = ts.store_path().join(out_path.to_string());
    let dev_disk = ts.store_path().join(dev_path.to_string());
    assert!(out_disk.exists(), "out output should exist on disk");
    assert!(dev_disk.exists(), "dev output should exist on disk");

    assert_eq!(std::fs::read_to_string(&out_disk).unwrap().trim(), "main");
    assert_eq!(
        std::fs::read_to_string(&dev_disk).unwrap().trim(),
        "dev-content"
    );

    // Both outputs should be registered in DB
    let out_valid = store.is_valid_path(&out_path).await.unwrap();
    let dev_valid = store.is_valid_path(&dev_path).await.unwrap();
    assert!(out_valid, "out should be registered in DB");
    assert!(dev_valid, "dev should be registered in DB");

    // Both should have valid path info with NAR hash
    let out_info = store.query_path_info(&out_path).await.unwrap().unwrap();
    let dev_info = store.query_path_info(&dev_path).await.unwrap().unwrap();
    assert!(out_info.nar_size > 0);
    assert!(dev_info.nar_size > 0);
    // Hashes should differ since content differs
    assert_ne!(out_info.nar_hash, dev_info.nar_hash);
}

/// Builder exits non-zero → `MiscFailure` with exit code, output cleaned up.
#[tokio::test]
async fn test_build_derivation_exit_nonzero() {
    let ts = TestStore::new();
    let mut store = ts.handler.clone().handshake().await.unwrap();

    let output_path = StorePath::from_base_path("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-fail").unwrap();
    let drv_path = StorePath::from_base_path("zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz-fail.drv").unwrap();

    let mut outputs = BTreeMap::new();
    outputs.insert(
        OutputName::default(),
        DerivationOutput::InputAddressed(output_path.clone()),
    );

    // Builder creates partial output then exits non-zero
    let drv = DerivationT {
        name: "fail".parse().unwrap(),
        outputs,
        inputs: BTreeSet::new(),
        platform: "x86_64-linux".into(),
        builder: "/bin/sh".into(),
        args: vec!["-c".into(), "echo partial > $out; exit 42".into()],
        env: BTreeMap::new(),
        structured_attrs: None,
    };

    let result = store
        .build_derivation(&drv_path, &drv, BuildMode::Normal)
        .await
        .unwrap();

    match &result.inner {
        BuildResultInner::Failure(f) => {
            assert_eq!(f.status, FailureStatus::MiscFailure);
            let msg = String::from_utf8_lossy(&f.error_msg);
            assert!(
                msg.contains("exit code 42"),
                "Error should mention exit code: {msg}"
            );
        }
        BuildResultInner::Success(_) => {
            panic!("Expected failure, got success");
        }
    }

    // Output path should be cleaned up
    let disk_path = ts.store_path().join(output_path.to_string());
    assert!(
        !disk_path.exists(),
        "Output should be cleaned up after failure"
    );

    // Not registered in DB
    let is_valid = store.is_valid_path(&output_path).await.unwrap();
    assert!(!is_valid, "Failed output should not be in DB");
}

/// Builder fails with `keepFailed = true` → output preserved with `.failed` suffix.
#[tokio::test]
async fn test_build_derivation_keep_failed() {
    let ts = TestStore::new();

    let output_path =
        StorePath::from_base_path("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-keepfail").unwrap();
    let drv_path =
        StorePath::from_base_path("zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz-keepfail.drv").unwrap();

    let mut outputs = BTreeMap::new();
    outputs.insert(
        OutputName::default(),
        DerivationOutput::InputAddressed(output_path.clone()),
    );

    let drv = DerivationT {
        name: "keepfail".parse().unwrap(),
        outputs,
        inputs: BTreeSet::new(),
        platform: "x86_64-linux".into(),
        builder: "/bin/sh".into(),
        args: vec!["-c".into(), "echo partial-output > $out; exit 1".into()],
        env: BTreeMap::new(),
        structured_attrs: None,
    };

    let config = crate::build::BuildConfig {
        keep_failed: true,
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
            assert_eq!(f.status, FailureStatus::MiscFailure);
        }
        BuildResultInner::Success(_) => {
            panic!("Expected failure, got success");
        }
    }

    // Original output path should NOT exist
    let disk_path = ts.store_path().join(output_path.to_string());
    assert!(!disk_path.exists(), "Original output should not exist");

    // .failed path SHOULD exist with partial content
    let failed_path = disk_path.with_file_name(format!("{}.failed", output_path));
    assert!(
        failed_path.exists(),
        "Failed output should be preserved at {}",
        failed_path.display()
    );
    let content = std::fs::read_to_string(&failed_path).unwrap();
    assert_eq!(content.trim(), "partial-output");
}

/// Missing input path → `MiscFailure` without spawning a process.
#[tokio::test]
async fn test_build_derivation_missing_input() {
    let ts = TestStore::new();
    let mut store = ts.handler.clone().handshake().await.unwrap();

    let output_path =
        StorePath::from_base_path("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-needsinput").unwrap();
    let drv_path =
        StorePath::from_base_path("zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz-needsinput.drv").unwrap();
    let missing_input =
        StorePath::from_base_path("mmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmm-missing").unwrap();

    let mut outputs = BTreeMap::new();
    outputs.insert(
        OutputName::default(),
        DerivationOutput::InputAddressed(output_path.clone()),
    );

    let mut inputs = BTreeSet::new();
    inputs.insert(missing_input);

    let drv = DerivationT {
        name: "needsinput".parse().unwrap(),
        outputs,
        inputs,
        platform: "x86_64-linux".into(),
        builder: "/bin/sh".into(),
        args: vec!["-c".into(), "echo hello > $out".into()],
        env: BTreeMap::new(),
        structured_attrs: None,
    };

    let result = store
        .build_derivation(&drv_path, &drv, BuildMode::Normal)
        .await
        .unwrap();

    match &result.inner {
        BuildResultInner::Failure(f) => {
            assert_eq!(f.status, FailureStatus::MiscFailure);
            let msg = String::from_utf8_lossy(&f.error_msg);
            assert!(
                msg.contains("missing input"),
                "Error should mention missing input: {msg}"
            );
        }
        BuildResultInner::Success(_) => {
            panic!("Expected failure for missing input, got success");
        }
    }

    // Output should not exist
    let disk_path = ts.store_path().join(output_path.to_string());
    assert!(!disk_path.exists(), "No output should be created");
}

/// Builder env contains standard Nix variables and derivation's custom env vars.
#[tokio::test]
async fn test_build_derivation_environment() {
    let ts = TestStore::new();
    let mut store = ts.handler.clone().handshake().await.unwrap();

    let output_path =
        StorePath::from_base_path("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-envtest").unwrap();
    let drv_path =
        StorePath::from_base_path("zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz-envtest.drv").unwrap();

    let mut outputs = BTreeMap::new();
    outputs.insert(
        OutputName::default(),
        DerivationOutput::InputAddressed(output_path.clone()),
    );

    // Builder prints specific env vars to $out using shell builtins
    // (can't use `env` command since PATH=/path-not-set)
    let mut env = BTreeMap::new();
    env.insert("MY_CUSTOM_VAR".into(), "custom_value".into());

    let script = r#"
printf 'HOME=%s\n' "$HOME" > "$out"
printf 'TERM=%s\n' "$TERM" >> "$out"
printf 'MY_CUSTOM_VAR=%s\n' "$MY_CUSTOM_VAR" >> "$out"
printf 'NIX_BUILD_TOP=%s\n' "$NIX_BUILD_TOP" >> "$out"
printf 'PATH=%s\n' "$PATH" >> "$out"
printf 'NIX_STORE=%s\n' "$NIX_STORE" >> "$out"
printf 'NIX_LOG_FD=%s\n' "$NIX_LOG_FD" >> "$out"
"#;

    let drv = DerivationT {
        name: "envtest".parse().unwrap(),
        outputs,
        inputs: BTreeSet::new(),
        platform: "x86_64-linux".into(),
        builder: "/bin/sh".into(),
        args: vec!["-c".into(), script.into()],
        env,
        structured_attrs: None,
    };

    let result = store
        .build_derivation(&drv_path, &drv, BuildMode::Normal)
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

    let disk_path = ts.store_path().join(output_path.to_string());
    let env_output = std::fs::read_to_string(&disk_path).unwrap();

    // Parse env output into a map
    let env_map: BTreeMap<&str, &str> = env_output
        .lines()
        .filter_map(|line| line.split_once('='))
        .collect();

    assert_eq!(env_map.get("HOME"), Some(&"/homeless-shelter"));
    assert_eq!(env_map.get("TERM"), Some(&"xterm-256color"));
    assert_eq!(env_map.get("MY_CUSTOM_VAR"), Some(&"custom_value"));
    assert_eq!(env_map.get("NIX_LOG_FD"), Some(&"2"));
    assert!(
        env_map.get("NIX_BUILD_TOP").is_some_and(|v| !v.is_empty()),
        "NIX_BUILD_TOP should be set to the build dir"
    );
    assert!(
        env_map.get("NIX_STORE").is_some_and(|v| !v.is_empty()),
        "NIX_STORE should be set"
    );
}

/// Multi-output derivation → builder sees `out`, `dev` env vars and `outputs=out dev`.
#[tokio::test]
async fn test_build_derivation_multi_output_env() {
    let ts = TestStore::new();
    let mut store = ts.handler.clone().handshake().await.unwrap();

    let out_path = StorePath::from_base_path("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-moenv").unwrap();
    let dev_path = StorePath::from_base_path("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-moenv-dev").unwrap();
    let drv_path = StorePath::from_base_path("zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz-moenv.drv").unwrap();

    let mut outputs = BTreeMap::new();
    outputs.insert(
        OutputName::default(),
        DerivationOutput::InputAddressed(out_path.clone()),
    );
    outputs.insert(
        "dev".parse().unwrap(),
        DerivationOutput::InputAddressed(dev_path.clone()),
    );

    // Print the output-related env vars into $out
    let script = r#"
printf 'out=%s\n' "$out" > "$out"
printf 'dev=%s\n' "$dev" >> "$out"
printf 'outputs=%s\n' "$outputs" >> "$out"
printf 'done\n' > "$dev"
"#;

    let drv = DerivationT {
        name: "moenv".parse().unwrap(),
        outputs,
        inputs: BTreeSet::new(),
        platform: "x86_64-linux".into(),
        builder: "/bin/sh".into(),
        args: vec!["-c".into(), script.into()],
        env: BTreeMap::new(),
        structured_attrs: None,
    };

    let result = store
        .build_derivation(&drv_path, &drv, BuildMode::Normal)
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

    let disk_path = ts.store_path().join(out_path.to_string());
    let content = std::fs::read_to_string(&disk_path).unwrap();
    let env_map: BTreeMap<&str, &str> = content
        .lines()
        .filter_map(|line| line.split_once('='))
        .collect();

    // `out` and `dev` should point to full store paths
    let out_val = env_map.get("out").expect("out env var should be set");
    let dev_val = env_map.get("dev").expect("dev env var should be set");
    assert!(
        out_val.contains(&out_path.to_string()),
        "out should contain store path: {out_val}"
    );
    assert!(
        dev_val.contains(&dev_path.to_string()),
        "dev should contain store path: {dev_val}"
    );

    // `outputs` should list both output names
    let outputs_val = env_map
        .get("outputs")
        .expect("outputs env var should be set");
    assert!(
        outputs_val.contains("out"),
        "outputs should contain 'out': {outputs_val}"
    );
    assert!(
        outputs_val.contains("dev"),
        "outputs should contain 'dev': {outputs_val}"
    );
}

/// Builder that exceeds wall-clock timeout → killed, `TimedOut`.
#[tokio::test]
async fn test_build_derivation_timeout() {
    let ts = TestStore::new();

    let output_path = StorePath::from_base_path("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-slow").unwrap();
    let drv_path = StorePath::from_base_path("zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz-slow.drv").unwrap();

    let mut outputs = BTreeMap::new();
    outputs.insert(
        OutputName::default(),
        DerivationOutput::InputAddressed(output_path.clone()),
    );

    let drv = DerivationT {
        name: "slow".parse().unwrap(),
        outputs,
        inputs: BTreeSet::new(),
        platform: "x86_64-linux".into(),
        builder: "/bin/sh".into(),
        args: vec!["-c".into(), "/bin/sleep 60; echo done > $out".into()],
        env: BTreeMap::new(),
        structured_attrs: None,
    };

    let config = crate::build::BuildConfig {
        timeout: Some(std::time::Duration::from_millis(100)),
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
            assert_eq!(f.status, FailureStatus::TimedOut);
        }
        BuildResultInner::Success(_) => {
            panic!("Expected timeout failure, got success");
        }
    }
}

/// Builder produces no output for longer than `max_silent` → killed, `TimedOut`.
#[tokio::test]
async fn test_build_derivation_max_silent() {
    let ts = TestStore::new();

    let output_path = StorePath::from_base_path("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-silent").unwrap();
    let drv_path =
        StorePath::from_base_path("zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz-silent.drv").unwrap();

    let mut outputs = BTreeMap::new();
    outputs.insert(
        OutputName::default(),
        DerivationOutput::InputAddressed(output_path.clone()),
    );

    // Builder sleeps without producing any output
    let drv = DerivationT {
        name: "silent".parse().unwrap(),
        outputs,
        inputs: BTreeSet::new(),
        platform: "x86_64-linux".into(),
        builder: "/bin/sh".into(),
        args: vec!["-c".into(), "/bin/sleep 60".into()],
        env: BTreeMap::new(),
        structured_attrs: None,
    };

    let config = crate::build::BuildConfig {
        max_silent_time: Some(std::time::Duration::from_millis(100)),
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
            assert_eq!(f.status, FailureStatus::TimedOut);
        }
        BuildResultInner::Success(_) => {
            panic!("Expected max_silent timeout, got success");
        }
    }
}

/// Log output from builder arrives in log_sink before `build_derivation` returns.
#[tokio::test]
async fn test_build_derivation_log_output() {
    let ts = TestStore::new();

    let output_path =
        StorePath::from_base_path("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-logtest").unwrap();
    let drv_path =
        StorePath::from_base_path("zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz-logtest.drv").unwrap();

    let mut outputs = BTreeMap::new();
    outputs.insert(
        OutputName::default(),
        DerivationOutput::InputAddressed(output_path.clone()),
    );

    // Builder writes log messages to stderr and output to $out
    let drv = DerivationT {
        name: "logtest".parse().unwrap(),
        outputs,
        inputs: BTreeSet::new(),
        platform: "x86_64-linux".into(),
        builder: "/bin/sh".into(),
        args: vec![
            "-c".into(),
            "echo 'log line 1' >&2; echo 'log line 2' >&2; echo done > $out".into(),
        ],
        env: BTreeMap::new(),
        structured_attrs: None,
    };

    let log_tmp = tempfile::tempdir().unwrap();
    let config = crate::build::BuildConfig {
        build_dir: ts.build_dir(),
        log_dir: Some(log_tmp.path().to_owned()),
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

    assert!(
        matches!(&result.inner, BuildResultInner::Success(_)),
        "Build should succeed"
    );

    // Read back the bzip2-compressed log file and verify contents
    let base_name = drv_path.to_string();
    let log_path = log_tmp
        .path()
        .join("drvs")
        .join(&base_name[..2])
        .join(format!("{}.bz2", &base_name[2..]));
    assert!(log_path.exists(), "Log file should exist at {log_path:?}");

    let compressed = std::fs::read(&log_path).unwrap();
    let mut decompressor = bzip2::read::BzDecoder::new(&compressed[..]);
    let mut log_text = String::new();
    std::io::Read::read_to_string(&mut decompressor, &mut log_text).unwrap();

    assert!(
        log_text.contains("log line 1"),
        "Log should contain 'log line 1': {log_text:?}",
    );
    assert!(
        log_text.contains("log line 2"),
        "Log should contain 'log line 2': {log_text:?}",
    );
}

/// `BuildMode::Repair` rebuilds even when output already exists.
#[tokio::test]
async fn test_build_derivation_repair_mode() {
    let ts = TestStore::new();
    let mut store = ts.handler.clone().handshake().await.unwrap();

    let (drv_path, drv) = simple_echo_derivation(&ts, "repair");

    // First build — should succeed normally
    let result = store
        .build_derivation(&drv_path, &drv, BuildMode::Normal)
        .await
        .unwrap();
    assert!(
        matches!(&result.inner, BuildResultInner::Success(s) if s.status == SuccessStatus::Built)
    );

    // Second build in Normal mode — should skip (AlreadyValid)
    let result = store
        .build_derivation(&drv_path, &drv, BuildMode::Normal)
        .await
        .unwrap();
    assert!(
        matches!(&result.inner, BuildResultInner::Success(s) if s.status == SuccessStatus::AlreadyValid),
        "Normal mode should return AlreadyValid for existing output"
    );

    // Repair mode — should rebuild even though output exists
    let result = store
        .build_derivation(&drv_path, &drv, BuildMode::Repair)
        .await
        .unwrap();
    match &result.inner {
        BuildResultInner::Success(s) => {
            assert_eq!(
                s.status,
                SuccessStatus::Built,
                "Repair should rebuild, not return AlreadyValid"
            );
        }
        BuildResultInner::Failure(f) => {
            panic!(
                "Repair build failed: {:?} - {}",
                f.status,
                String::from_utf8_lossy(&f.error_msg)
            );
        }
    }

    // Output should still be valid
    let output_path = StorePath::from_base_path("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-repair").unwrap();
    let is_valid = store.is_valid_path(&output_path).await.unwrap();
    assert!(is_valid, "Output should still be registered after repair");
}

/// `BuildMode::Check` builds but does not modify the store; mismatched hash
/// reports non-determinism.
#[tokio::test]
async fn test_build_derivation_check_mode() {
    let ts = TestStore::new();
    let mut store = ts.handler.clone().handshake().await.unwrap();

    // First build with Normal mode to establish the baseline
    let (drv_path, drv) = simple_echo_derivation(&ts, "checkme");

    let result = store
        .build_derivation(&drv_path, &drv, BuildMode::Normal)
        .await
        .unwrap();
    assert!(
        matches!(&result.inner, BuildResultInner::Success(s) if s.status == SuccessStatus::Built)
    );

    // Get the registered hash
    let output_path =
        StorePath::from_base_path("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-checkme").unwrap();
    let info_before = store.query_path_info(&output_path).await.unwrap().unwrap();

    // Check mode should rebuild and compare but not modify the store
    let result = store
        .build_derivation(&drv_path, &drv, BuildMode::Check)
        .await
        .unwrap();

    // Same derivation → deterministic → should succeed
    match &result.inner {
        BuildResultInner::Success(s) => {
            assert_eq!(s.status, SuccessStatus::Built);
        }
        BuildResultInner::Failure(f) => {
            panic!(
                "Check mode failed unexpectedly: {:?} - {}",
                f.status,
                String::from_utf8_lossy(&f.error_msg)
            );
        }
    }

    // Store should be unmodified — same hash, same path info
    let info_after = store.query_path_info(&output_path).await.unwrap().unwrap();
    assert_eq!(info_before.nar_hash, info_after.nar_hash);
    assert_eq!(info_before.nar_size, info_after.nar_size);
}

/// Derivation env can override PATH (set before drv env) but cannot override
/// TMPDIR (set after drv env), matching Nix's `initEnv()` ordering.
#[tokio::test]
async fn test_build_derivation_env_override_semantics() {
    let ts = TestStore::new();
    let mut store = ts.handler.clone().handshake().await.unwrap();

    let output_path =
        StorePath::from_base_path("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-envorder").unwrap();
    let drv_path =
        StorePath::from_base_path("zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz-envorder.drv").unwrap();

    let mut outputs = BTreeMap::new();
    outputs.insert(
        OutputName::default(),
        DerivationOutput::InputAddressed(output_path.clone()),
    );

    // Derivation tries to override both PATH (should work) and TMPDIR (should not)
    let mut env = BTreeMap::new();
    env.insert("PATH".into(), "/custom/path".into());
    env.insert("TMPDIR".into(), "/should/be/ignored".into());

    let script = r#"
printf 'PATH=%s\n' "$PATH" > "$out"
printf 'TMPDIR=%s\n' "$TMPDIR" >> "$out"
"#;

    let drv = DerivationT {
        name: "envorder".parse().unwrap(),
        outputs,
        inputs: BTreeSet::new(),
        platform: "x86_64-linux".into(),
        builder: "/bin/sh".into(),
        args: vec!["-c".into(), script.into()],
        env,
        structured_attrs: None,
    };

    let result = store
        .build_derivation(&drv_path, &drv, BuildMode::Normal)
        .await
        .unwrap();

    assert!(
        matches!(&result.inner, BuildResultInner::Success(_)),
        "Build should succeed"
    );

    let disk_path = ts.store_path().join(output_path.to_string());
    let content = std::fs::read_to_string(&disk_path).unwrap();
    let env_map: BTreeMap<&str, &str> = content
        .lines()
        .filter_map(|line| line.split_once('='))
        .collect();

    // PATH should be overridden by derivation env (set before drv env in Nix)
    assert_eq!(
        env_map.get("PATH"),
        Some(&"/custom/path"),
        "Derivation should be able to override PATH"
    );

    // TMPDIR should NOT be overridden (set after drv env in Nix)
    assert_ne!(
        env_map.get("TMPDIR"),
        Some(&"/should/be/ignored"),
        "Derivation should NOT be able to override TMPDIR"
    );
}

/// passAsFile writes env var contents to files, replacing `text` with `textPath`.
#[tokio::test]
async fn test_build_derivation_pass_as_file() {
    let ts = TestStore::new();
    let mut store = ts.handler.clone().handshake().await.unwrap();

    let output_path =
        StorePath::from_base_path("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-passfile").unwrap();
    let drv_path =
        StorePath::from_base_path("zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz-passfile.drv").unwrap();

    let mut outputs = BTreeMap::new();
    outputs.insert(
        OutputName::default(),
        DerivationOutput::InputAddressed(output_path.clone()),
    );

    // The builder reads the file pointed to by $textPath, verifies `text` is
    // NOT in the env, and writes the file contents to $out.
    // Use shell built-ins only — only /bin/sh exists in the Nix sandbox.
    let script = r#"
        if [ -n "${text+set}" ]; then
            echo "FAIL: text env var should not be set" >&2
            exit 1
        fi
        if [ -z "$textPath" ]; then
            echo "FAIL: textPath env var not set" >&2
            exit 1
        fi
        if [ ! -f "$textPath" ]; then
            echo "FAIL: textPath does not point to a file" >&2
            exit 1
        fi
        IFS= read -r content < "$textPath"
        printf '%s' "$content" > "$out"
    "#;

    let mut env = BTreeMap::new();
    env.insert("passAsFile".into(), "text".into());
    env.insert("text".into(), "hello from passAsFile".into());

    let drv = DerivationT {
        name: "passfile".parse().unwrap(),
        outputs,
        inputs: BTreeSet::new(),
        platform: "x86_64-linux".into(),
        builder: "/bin/sh".into(),
        args: vec!["-c".into(), script.into()],
        env,
        structured_attrs: None,
    };

    let result = store
        .build_derivation(&drv_path, &drv, BuildMode::Normal)
        .await
        .unwrap();

    assert!(
        matches!(&result.inner, BuildResultInner::Success(_)),
        "Build should succeed, got: {:?}",
        result.inner
    );

    let disk_path = ts.store_path().join(output_path.to_string());
    let content = std::fs::read_to_string(&disk_path).unwrap();
    assert_eq!(
        content, "hello from passAsFile",
        "Output should contain the passAsFile content"
    );
}

/// structuredAttrs writes `.attrs.json` in build dir, sets `NIX_ATTRS_JSON_FILE`,
/// and does NOT set individual derivation env vars.
#[tokio::test]
async fn test_build_derivation_structured_attrs() {
    let ts = TestStore::new();
    let mut store = ts.handler.clone().handshake().await.unwrap();

    let output_path = StorePath::from_base_path("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-sattrs").unwrap();
    let drv_path =
        StorePath::from_base_path("zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz-sattrs.drv").unwrap();

    let mut outputs = BTreeMap::new();
    outputs.insert(
        OutputName::default(),
        DerivationOutput::InputAddressed(output_path.clone()),
    );

    // The builder verifies:
    // 1. NIX_ATTRS_JSON_FILE is set and points to a readable file
    // 2. The JSON file contains our custom attribute
    // 3. Individual env vars from the derivation are NOT set
    //
    // Use shell built-ins only — only /bin/sh exists on NixOS.
    let script = r#"
        if [ -z "$NIX_ATTRS_JSON_FILE" ]; then
            echo "FAIL: NIX_ATTRS_JSON_FILE not set" >&2
            exit 1
        fi
        if [ ! -f "$NIX_ATTRS_JSON_FILE" ]; then
            echo "FAIL: NIX_ATTRS_JSON_FILE does not point to a file" >&2
            exit 1
        fi
        if [ -n "${myAttr+set}" ]; then
            echo "FAIL: myAttr env var should NOT be set in structured mode" >&2
            exit 1
        fi
        IFS= read -r content < "$NIX_ATTRS_JSON_FILE"
        printf '%s' "$content" > "$out"
    "#;

    let mut env = BTreeMap::new();
    env.insert("__structuredAttrs".into(), "1".into());
    env.insert("myAttr".into(), "myValue".into());

    let mut structured_attrs_map = serde_json::Map::new();
    structured_attrs_map.insert("myAttr".into(), serde_json::Value::String("myValue".into()));

    let drv = DerivationT {
        name: "sattrs".parse().unwrap(),
        outputs,
        inputs: BTreeSet::new(),
        platform: "x86_64-linux".into(),
        builder: "/bin/sh".into(),
        args: vec!["-c".into(), script.into()],
        env,
        structured_attrs: Some(StructuredAttrs {
            attrs: structured_attrs_map,
        }),
    };

    let result = store
        .build_derivation(&drv_path, &drv, BuildMode::Normal)
        .await
        .unwrap();

    assert!(
        matches!(&result.inner, BuildResultInner::Success(_)),
        "Build should succeed, got: {:?}",
        result.inner
    );

    // Verify the JSON file contents written to the output
    let disk_path = ts.store_path().join(output_path.to_string());
    let content = std::fs::read_to_string(&disk_path).unwrap();
    let json: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert_eq!(
        json.get("myAttr").and_then(serde_json::Value::as_str),
        Some("myValue"),
        "JSON should contain myAttr"
    );
}

/// structuredAttrs with multiple outputs → `.attrs.json` contains `outputs` map.
#[tokio::test]
async fn test_build_derivation_structured_attrs_outputs() {
    let ts = TestStore::new();
    let mut store = ts.handler.clone().handshake().await.unwrap();

    let out_path = StorePath::from_base_path("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-saout").unwrap();
    let dev_path = StorePath::from_base_path("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-saout-dev").unwrap();
    let drv_path = StorePath::from_base_path("zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz-saout.drv").unwrap();

    let mut outputs = BTreeMap::new();
    outputs.insert(
        OutputName::default(),
        DerivationOutput::InputAddressed(out_path.clone()),
    );
    outputs.insert(
        "dev".parse().unwrap(),
        DerivationOutput::InputAddressed(dev_path.clone()),
    );

    // Builder reads .attrs.json, extracts the outputs map, and verifies both
    // "out" and "dev" keys are present with store path values.
    let script = r#"
        if [ -z "$NIX_ATTRS_JSON_FILE" ]; then
            echo "FAIL: NIX_ATTRS_JSON_FILE not set" >&2
            exit 1
        fi
        # Write the JSON to $out for inspection by the test
        IFS= read -r content < "$NIX_ATTRS_JSON_FILE"
        printf '%s' "$content" > "$out"
        echo ok > $dev
    "#;

    let mut env = BTreeMap::new();
    env.insert("__structuredAttrs".into(), "1".into());

    let drv = DerivationT {
        name: "saout".parse().unwrap(),
        outputs,
        inputs: BTreeSet::new(),
        platform: "x86_64-linux".into(),
        builder: "/bin/sh".into(),
        args: vec!["-c".into(), script.into()],
        env,
        structured_attrs: Some(StructuredAttrs {
            attrs: serde_json::Map::new(),
        }),
    };

    let result = store
        .build_derivation(&drv_path, &drv, BuildMode::Normal)
        .await
        .unwrap();

    assert!(
        matches!(&result.inner, BuildResultInner::Success(_)),
        "Build should succeed, got: {:?}",
        result.inner
    );

    let disk_path = ts.store_path().join(out_path.to_string());
    let content = std::fs::read_to_string(&disk_path).unwrap();
    let json: serde_json::Value = serde_json::from_str(&content).unwrap();

    let outputs_obj = json
        .get("outputs")
        .expect("JSON should have 'outputs' key")
        .as_object()
        .expect("'outputs' should be an object");

    let out_val = outputs_obj
        .get("out")
        .and_then(serde_json::Value::as_str)
        .expect("outputs.out should be a string");
    assert!(
        out_val.contains(&out_path.to_string()),
        "outputs.out should contain the out store path, got: {out_val}"
    );

    let dev_val = outputs_obj
        .get("dev")
        .and_then(serde_json::Value::as_str)
        .expect("outputs.dev should be a string");
    assert!(
        dev_val.contains(&dev_path.to_string()),
        "outputs.dev should contain the dev store path, got: {dev_val}"
    );
}
