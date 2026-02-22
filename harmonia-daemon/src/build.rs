// SPDX-FileCopyrightText: 2026 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! Build derivation executor.
//!
//! This module implements the core build logic for `build_derivation`:
//! validate inputs, prepare the build environment, execute the builder
//! process, scan outputs for references, canonicalize metadata, compute
//! NAR hashes, and register outputs in the database.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use futures::StreamExt as _;
use tokio::io::AsyncBufReadExt;
use tokio::sync::Mutex;

use harmonia_protocol::NarHash;
use harmonia_protocol::build_result::{
    BuildResult, BuildResultFailure, BuildResultInner, BuildResultSuccess, FailureStatus,
    SuccessStatus,
};
use harmonia_protocol::daemon::DaemonError as ProtocolError;
use harmonia_protocol::daemon::DaemonResult;
use harmonia_protocol::daemon_wire::types2::BuildMode;
use harmonia_store_core::derivation::{BasicDerivation, DerivationOutput};
use harmonia_store_core::derived_path::OutputName;
use harmonia_store_core::references::RefScanSink;
use harmonia_store_core::store_path::{StoreDir, StorePath};
use harmonia_utils_hash::fmt::CommonHash as _;
use harmonia_utils_hash::{Algorithm, Context};

use crate::canonicalize::canonicalize_path_metadata;
use crate::export_references_graph::{check_output_constraints, write_export_references_graph};
use crate::sandbox::Sandbox;

/// Default parent directory for build sandboxes, matching upstream Nix.
pub const DEFAULT_BUILD_DIR: &str = "/nix/var/nix/builds";

/// Default log directory, matching upstream Nix's `logDir` setting.
pub const DEFAULT_LOG_DIR: &str = "/nix/var/log/nix";

/// Configuration for a build operation.
pub struct BuildConfig {
    /// Whether to keep failed build outputs (with `.failed` suffix).
    pub keep_failed: bool,
    /// Wall-clock timeout for builds (None = no timeout).
    pub timeout: Option<std::time::Duration>,
    /// Max time without log output before killing the build (None = no limit).
    pub max_silent_time: Option<std::time::Duration>,
    /// Number of CPU cores available to the build.
    pub cores: usize,
    /// Parent directory for temporary build directories.
    /// Defaults to `/nix/var/nix/builds`.
    pub build_dir: PathBuf,
    /// Directory where bzip2-compressed build logs are written.
    /// Logs are stored as `{log_dir}/drvs/{hash[0:2]}/{hash[2:]}.bz2`.
    /// Set to `None` to disable log persistence (useful in tests).
    pub log_dir: Option<PathBuf>,
    /// Allowed prefixes for `impureHostDeps` (macOS).
    /// Paths in `__impureHostDeps` must start with one of these prefixes
    /// to be included in the sandbox.
    pub allowed_impure_host_deps: Vec<PathBuf>,
}

impl Default for BuildConfig {
    fn default() -> Self {
        Self {
            keep_failed: false,
            timeout: None,
            max_silent_time: None,
            cores: std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1),
            build_dir: PathBuf::from(DEFAULT_BUILD_DIR),
            log_dir: Some(PathBuf::from(DEFAULT_LOG_DIR)),
            allowed_impure_host_deps: vec![
                PathBuf::from("/usr/lib"),
                PathBuf::from("/System/Library"),
            ],
        }
    }
}

/// Open a bzip2-compressed build log file for the given derivation path.
///
/// Returns a `BzEncoder<File>` writer and the path to the log file.
/// Matches Nix's log layout: `{log_dir}/drvs/{base_name[0:2]}/{base_name[2:]}.bz2`
fn open_build_log(
    drv_path: &StorePath,
    config: &BuildConfig,
) -> std::io::Result<Option<(bzip2::write::BzEncoder<std::fs::File>, PathBuf)>> {
    let log_dir = match &config.log_dir {
        Some(d) => d,
        None => return Ok(None),
    };

    let base_name = drv_path.to_string();
    let (prefix, rest) = base_name.split_at(2);
    let dir = log_dir.join("drvs").join(prefix);
    std::fs::create_dir_all(&dir)?;

    let log_path = dir.join(format!("{rest}.bz2"));
    let file = std::fs::File::create(&log_path)?;
    let writer = bzip2::write::BzEncoder::new(file, bzip2::Compression::default());

    Ok(Some((writer, log_path)))
}

/// Result of scanning and hashing a single built output in one NAR pass.
#[derive(Clone)]
pub(crate) struct BuiltOutput {
    pub(crate) path: StorePath,
    pub(crate) nar_hash: NarHash,
    pub(crate) nar_size: u64,
    pub(crate) references: BTreeSet<StorePath>,
}

/// Execute a build for the given derivation and register outputs.
///
/// This is the core build logic called by `LocalStoreHandler::build_derivation`.
/// Build logs are written to `{config.log_dir}/drvs/...` (bzip2-compressed by
/// default), matching Nix's `addBuildLog()` layout so that `nix log` can read them.
pub async fn build_derivation(
    store_dir: &StoreDir,
    db: &Arc<Mutex<harmonia_store_db::StoreDb>>,
    drv_path: &StorePath,
    drv: &BasicDerivation,
    mode: BuildMode,
    config: &BuildConfig,
) -> DaemonResult<BuildResult> {
    let start_time = now_secs();

    // If mode is Normal, check if all outputs already exist
    if mode == BuildMode::Normal && check_all_outputs_exist(store_dir, db, drv).await? {
        return Ok(BuildResult {
            inner: BuildResultInner::Success(BuildResultSuccess {
                status: SuccessStatus::AlreadyValid,
                built_outputs: BTreeMap::new(),
            }),
            times_built: 0,
            start_time,
            stop_time: start_time,
            cpu_user: None,
            cpu_system: None,
        });
    }

    // Validate all input paths exist on disk
    if let Err(msg) = validate_inputs(store_dir, drv) {
        return Ok(make_failure(
            FailureStatus::MiscFailure,
            msg,
            start_time,
            now_secs(),
        ));
    }

    // Resolve output paths from the derivation
    let output_paths = resolve_output_paths(store_dir, drv)?;

    // Create temporary build directory under the configured build_dir.
    // We intentionally omit the derivation name to avoid exceeding filesystem
    // path-component length limits (NAME_MAX, typically 255 bytes).
    let build_tmp = tempfile::Builder::new()
        .prefix("nix-build-")
        .tempdir_in(&config.build_dir)
        .map_err(|e| {
            ProtocolError::custom(format!(
                "Failed to create build dir in {}: {e}",
                config.build_dir.display()
            ))
        })?;

    // Validate impure host deps (macOS) — fail early if not allowed
    if let Err(msg) = validate_impure_host_deps(drv, config) {
        return Ok(make_failure(
            FailureStatus::MiscFailure,
            msg,
            start_time,
            now_secs(),
        ));
    }

    // Build environment variables (includes passAsFile handling which writes files)
    let env = build_environment(store_dir, drv, build_tmp.path(), &output_paths, config)
        .map_err(|e| ProtocolError::custom(format!("Failed to set up build environment: {e}")))?;

    // Write exportReferencesGraph files to the build dir
    write_export_references_graph(store_dir, db, drv, build_tmp.path()).await?;

    // Extract builder path and args
    let builder = std::str::from_utf8(&drv.builder)
        .map_err(|e| ProtocolError::custom(format!("Invalid UTF-8 in builder: {e}")))?;
    let args: Vec<&str> = drv
        .args
        .iter()
        .map(|a| {
            std::str::from_utf8(a)
                .map_err(|e| ProtocolError::custom(format!("Invalid UTF-8 in args: {e}")))
        })
        .collect::<Result<_, _>>()?;

    // In Repair mode, remove existing output paths first
    if mode == BuildMode::Repair {
        for (_name, out_path) in &output_paths {
            let full_path = store_dir.to_path().join(out_path.to_string());
            let _ = tokio::fs::remove_dir_all(&full_path).await;
            let _ = tokio::fs::remove_file(&full_path).await;
        }
    }

    // Open build log file (bzip2-compressed by default, matching Nix's layout).
    // If log_dir is None the build log is simply discarded.
    let log_sink: Arc<std::sync::Mutex<dyn Write + Send>> = match open_build_log(drv_path, config) {
        Ok(Some((writer, _path))) => Arc::new(std::sync::Mutex::new(writer)),
        Ok(None) => Arc::new(std::sync::Mutex::new(std::io::sink())),
        Err(e) => {
            tracing::warn!("Failed to open build log for {drv_path}: {e}");
            Arc::new(std::sync::Mutex::new(std::io::sink()))
        }
    };

    // Dispatch to builtin builder or external process
    let build_result = if let Some(builtin_name) = builder.strip_prefix("builtin:") {
        run_builtin_builder(builtin_name, drv, &env, &output_paths, store_dir).await
    } else {
        execute_builder(builder, &args, &env, build_tmp.path(), config, &log_sink).await
    };

    // Flush / finalize the log file (important for bzip2 trailer)
    drop(log_sink);

    let stop_time = now_secs();

    // Convert build errors to failure descriptions; on success hand off to
    // process_build_success which registers outputs.
    let failure = match build_result {
        Err(BuildError::Timeout) => Some(make_failure(
            FailureStatus::TimedOut,
            "build timed out".to_string(),
            start_time,
            stop_time,
        )),
        Err(BuildError::ExitCode(code)) => Some(make_failure(
            FailureStatus::MiscFailure,
            format!("builder for '{}' failed with exit code {}", drv_path, code),
            start_time,
            stop_time,
        )),
        Err(BuildError::Other(msg)) => Some(make_failure(
            FailureStatus::MiscFailure,
            msg,
            start_time,
            stop_time,
        )),
        Ok(()) => None,
    };

    if let Some(result) = failure {
        cleanup_outputs(store_dir, &output_paths, config.keep_failed).await;
        return Ok(result);
    }

    process_build_success(
        store_dir,
        db,
        drv_path,
        drv,
        mode,
        &output_paths,
        (start_time, stop_time),
    )
    .await
}

/// After a successful builder exit, canonicalize outputs, scan references,
/// compute NAR hashes, and register (or check) outputs.
async fn process_build_success(
    store_dir: &StoreDir,
    db: &Arc<Mutex<harmonia_store_db::StoreDb>>,
    drv_path: &StorePath,
    drv: &BasicDerivation,
    mode: BuildMode,
    output_paths: &[(OutputName, StorePath)],
    times: (libc::time_t, libc::time_t),
) -> DaemonResult<BuildResult> {
    let (start_time, stop_time) = times;
    let repair = mode == BuildMode::Repair;
    if mode == BuildMode::Check {
        return check_mode_verify(store_dir, db, drv_path, output_paths, start_time, stop_time)
            .await;
    }

    // Normal / Repair: canonicalize, scan+hash, register
    let mut built_outputs = Vec::new();
    for (_name, out_path) in output_paths {
        let full_path = store_dir.to_path().join(out_path.to_string());

        if !full_path.exists() {
            return Ok(make_failure(
                FailureStatus::MiscFailure,
                format!(
                    "builder for '{}' failed to produce output path '{}'",
                    drv_path, out_path
                ),
                start_time,
                stop_time,
            ));
        }

        // Canonicalize metadata (permissions, timestamps, ownership)
        canonicalize_path_metadata(&full_path).await.map_err(|e| {
            ProtocolError::custom(format!("Failed to canonicalize {out_path}: {e}"))
        })?;

        // Single-pass: scan references AND compute NAR hash simultaneously
        let (nar_hash, nar_size, references) =
            hash_and_scan(&full_path, &drv.inputs, out_path).await?;

        built_outputs.push(BuiltOutput {
            path: out_path.clone(),
            nar_hash,
            nar_size,
            references,
        });
    }

    // Validate output reference constraints before registering
    if let Err(msg) = check_output_constraints(store_dir, db, drv, &built_outputs).await {
        cleanup_outputs(store_dir, output_paths, false).await;
        return Ok(make_failure(
            FailureStatus::OutputRejected,
            msg,
            start_time,
            stop_time,
        ));
    }

    // Register all outputs in the database (repair mode deletes existing entries first)
    register_outputs(store_dir, db, drv_path, &built_outputs, repair).await?;

    Ok(BuildResult {
        inner: BuildResultInner::Success(BuildResultSuccess {
            status: SuccessStatus::Built,
            // built_outputs is intentionally empty: it holds CA derivation
            // realisations (DrvOutput → Realisation), which only apply to
            // content-addressed derivations.  Input-addressed builds register
            // their output metadata (nar_hash, nar_size, references) in
            // ValidPaths via register_outputs() above, not as Realisations.
            built_outputs: BTreeMap::new(),
        }),
        times_built: 1,
        start_time,
        stop_time,
        cpu_user: None,
        cpu_system: None,
    })
}

/// In Check mode: build but don't register; compare hashes against existing.
async fn check_mode_verify(
    store_dir: &StoreDir,
    db: &Arc<Mutex<harmonia_store_db::StoreDb>>,
    drv_path: &StorePath,
    output_paths: &[(OutputName, StorePath)],
    start_time: libc::time_t,
    stop_time: libc::time_t,
) -> DaemonResult<BuildResult> {
    let mut non_deterministic = false;
    for (name, out_path) in output_paths {
        let full_path = store_dir.to_path().join(out_path.to_string());
        let (nar_hash, _nar_size, _refs) =
            hash_and_scan(&full_path, &BTreeSet::new(), out_path).await?;

        // Compare against registered hash
        let full_path_str = full_path.to_string_lossy().to_string();
        let db_guard = db.lock().await;
        if let Ok(Some(info)) = db_guard.query_path_info(&full_path_str) {
            let existing_hash: harmonia_utils_hash::fmt::Any<harmonia_utils_hash::Hash> =
                info.hash.parse().map_err(|e| {
                    ProtocolError::custom(format!("Failed to parse existing hash: {e}"))
                })?;
            let existing_nar_hash = NarHash::try_from(existing_hash.into_hash())
                .map_err(|e| ProtocolError::custom(format!("Hash conversion: {e}")))?;
            if existing_nar_hash != nar_hash {
                non_deterministic = true;
                tracing::warn!(
                    "Output {} of {} is not deterministic (hash mismatch)",
                    name,
                    drv_path
                );
            }
        }
        drop(db_guard);

        // Clean up the check build output
        let _ = tokio::fs::remove_dir_all(&full_path).await;
        let _ = tokio::fs::remove_file(&full_path).await;
    }

    if non_deterministic {
        return Ok(make_failure(
            FailureStatus::NotDeterministic,
            format!("derivation '{}' is not deterministic", drv_path),
            start_time,
            stop_time,
        ));
    }

    Ok(BuildResult {
        inner: BuildResultInner::Success(BuildResultSuccess {
            status: SuccessStatus::Built,
            built_outputs: BTreeMap::new(),
        }),
        times_built: 1,
        start_time,
        stop_time,
        cpu_user: None,
        cpu_system: None,
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Check if all outputs of a derivation already exist in the store DB.
async fn check_all_outputs_exist(
    store_dir: &StoreDir,
    db: &Arc<Mutex<harmonia_store_db::StoreDb>>,
    drv: &BasicDerivation,
) -> DaemonResult<bool> {
    for (name, output) in &drv.outputs {
        if let Some(path) = output
            .path(store_dir, &drv.name, name)
            .map_err(|e| ProtocolError::custom(format!("Invalid output path: {e}")))?
        {
            let full_path = format!("{}/{}", store_dir, path);
            let db = db.clone();
            let exists = tokio::task::spawn_blocking(move || {
                let db = db.blocking_lock();
                db.is_valid_path(&full_path)
            })
            .await
            .map_err(|e| ProtocolError::custom(format!("Task join error: {e}")))?
            .map_err(|e| ProtocolError::custom(format!("Database error: {e}")))?;

            if !exists {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

/// Validate that all input store paths exist on disk.
fn validate_inputs(store_dir: &StoreDir, drv: &BasicDerivation) -> Result<(), String> {
    for input_path in &drv.inputs {
        let full_path = store_dir.to_path().join(input_path.to_string());
        if !full_path.exists() {
            return Err(format!("missing input store path '{}'", input_path));
        }
    }
    Ok(())
}

/// Resolve output names → store paths from the derivation.
fn resolve_output_paths(
    store_dir: &StoreDir,
    drv: &BasicDerivation,
) -> DaemonResult<Vec<(OutputName, StorePath)>> {
    let mut result = Vec::new();
    for (name, output) in &drv.outputs {
        match output.path(store_dir, &drv.name, name) {
            Ok(Some(path)) => result.push((name.clone(), path)),
            Ok(None) => {
                return Err(ProtocolError::custom(format!(
                    "cannot determine output path for output '{name}'"
                )));
            }
            Err(e) => {
                return Err(ProtocolError::custom(format!(
                    "invalid output path for '{name}': {e}"
                )));
            }
        }
    }
    Ok(result)
}

/// Build the environment variables map for the builder process.
///
/// Matches Nix's `initEnv()` ordering exactly. Uses a `BTreeMap` so that
/// last-write-wins semantics apply (same as Nix's `std::map`).
///
/// Order (matching `derivation-builder.cc`):
/// 1. `PATH`, `HOME`, `NIX_STORE`, `NIX_BUILD_CORES` — can be overridden by drv env
/// 2. Derivation env vars — can override (1) but not (3)
/// 3. `NIX_BUILD_TOP`, `TMPDIR`/`TEMPDIR`/`TMP`/`TEMP`, `PWD` — cannot be overridden
/// 4. `NIX_OUTPUT_CHECKED` (fixed-output only)
/// 5. `NIX_LOG_FD`, `TERM` — cannot be overridden
fn build_environment(
    store_dir: &StoreDir,
    drv: &BasicDerivation,
    build_dir: &Path,
    output_paths: &[(OutputName, StorePath)],
    config: &BuildConfig,
) -> std::io::Result<BTreeMap<String, String>> {
    let mut env = BTreeMap::new();
    let build_dir_str = build_dir.to_string_lossy().to_string();
    let is_structured = drv.structured_attrs.is_some();

    // Phase 1: defaults that CAN be overridden by derivation env
    env.insert("PATH".into(), "/path-not-set".into());
    env.insert("HOME".into(), "/homeless-shelter".into());
    env.insert("NIX_STORE".into(), store_dir.to_str().to_string());
    env.insert("NIX_BUILD_CORES".into(), config.cores.to_string());

    if is_structured {
        // Structured attrs mode: write .attrs.json to the build dir.
        // Individual derivation env vars are NOT set — the builder reads
        // everything from the JSON file.
        let json = prepare_structured_attrs(drv, store_dir, output_paths);
        let json_str = serde_json::to_string(&json)
            .map_err(|e| std::io::Error::other(format!("JSON serialization: {e}")))?;
        let json_path = build_dir.join(".attrs.json");
        std::fs::write(&json_path, &json_str)?;
        env.insert(
            "NIX_ATTRS_JSON_FILE".into(),
            json_path.to_string_lossy().to_string(),
        );
    } else {
        // Non-structured mode: set derivation env vars, honoring passAsFile.
        let pass_as_file: BTreeSet<String> = drv
            .env
            .get(b"passAsFile".as_ref())
            .map(|v| {
                String::from_utf8_lossy(v)
                    .split_whitespace()
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();

        for (key, value) in &drv.env {
            let key_str = String::from_utf8_lossy(key).to_string();
            let value_str = String::from_utf8_lossy(value).to_string();

            if key_str == "passAsFile" {
                continue;
            }

            if pass_as_file.contains(&key_str) {
                // Write value to a file in the build dir, matching Nix's naming:
                // .attr-<sha256(name)> (nix32-encoded)
                let name_hash = {
                    let mut ctx = Context::new(Algorithm::SHA256);
                    ctx.update(key_str.as_bytes());
                    ctx.finish()
                };
                let file_name = format!(".attr-{}", name_hash.as_base32());
                let file_path = build_dir.join(&file_name);
                std::fs::write(&file_path, value.as_ref())?;
                env.insert(
                    format!("{key_str}Path"),
                    file_path.to_string_lossy().to_string(),
                );
            } else {
                env.insert(key_str, value_str);
            }
        }
    }

    // Phase 3: system vars set AFTER drv env (cannot be overridden)
    env.insert("NIX_BUILD_TOP".into(), build_dir_str.clone());
    env.insert("TMPDIR".into(), build_dir_str.clone());
    env.insert("TEMPDIR".into(), build_dir_str.clone());
    env.insert("TMP".into(), build_dir_str.clone());
    env.insert("TEMP".into(), build_dir_str.clone());
    env.insert("PWD".into(), build_dir_str);

    // Output path variables and `outputs` list
    let mut output_names: Vec<String> = Vec::new();
    for (name, path) in output_paths {
        let full_path = store_dir.to_path().join(path.to_string());
        env.insert(name.to_string(), full_path.to_string_lossy().to_string());
        output_names.push(name.to_string());
    }
    env.insert("outputs".into(), output_names.join(" "));

    // Phase 4: fixed-output derivation env vars
    if is_fixed_output(drv) {
        env.insert("NIX_OUTPUT_CHECKED".into(), "1".into());
        // Propagate impure env vars from the process environment
        // (e.g., http_proxy for fetchurl)
        if let Some(impure_vars) = drv.env.get(b"impureEnvVars".as_ref()) {
            let var_names = String::from_utf8_lossy(impure_vars);
            for var_name in var_names.split_whitespace() {
                if let Ok(val) = std::env::var(var_name) {
                    env.insert(var_name.to_string(), val);
                }
            }
        }
    }

    // Phase 5: final system vars (cannot be overridden)
    env.insert("NIX_LOG_FD".into(), "2".into());
    env.insert("TERM".into(), "xterm-256color".into());

    Ok(env)
}

/// Check if a derivation is a fixed-output derivation.
///
/// A fixed-output derivation has exactly one output and that output is `CAFixed`.
fn is_fixed_output(drv: &BasicDerivation) -> bool {
    drv.outputs.len() == 1
        && drv
            .outputs
            .values()
            .next()
            .is_some_and(|o| matches!(o, DerivationOutput::CAFixed(_)))
}

/// Validate and collect impure host deps from a derivation.
///
/// Returns the list of allowed host deps as `(path, optional)` pairs, where
/// `optional` is true (missing paths are tolerated). Returns an error if any
/// path is not within the allowed prefixes.
///
/// On non-macOS platforms, this is a no-op (impureHostDeps are macOS-specific).
pub fn validate_impure_host_deps(
    drv: &BasicDerivation,
    config: &BuildConfig,
) -> Result<Vec<(PathBuf, bool)>, String> {
    let mut result = Vec::new();

    // Collect deps from env or structured attrs
    let deps: Vec<String> = if let Some(val) = drv.env.get(b"__impureHostDeps".as_ref()) {
        String::from_utf8_lossy(val)
            .split_whitespace()
            .map(String::from)
            .collect()
    } else if let Some(sa) = &drv.structured_attrs {
        if let Some(serde_json::Value::Array(arr)) = sa.attrs.get("__impureHostDeps") {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };

    for dep in deps {
        let dep_path = PathBuf::from(&dep);
        let allowed = config
            .allowed_impure_host_deps
            .iter()
            .any(|prefix| dep_path.starts_with(prefix));
        if !allowed {
            return Err(format!(
                "impure host dep '{}' is not within any allowed prefix ({:?})",
                dep, config.allowed_impure_host_deps
            ));
        }
        // All impure host deps are optional (missing is tolerated)
        result.push((dep_path, true));
    }

    Ok(result)
}

/// Build the JSON object for `.attrs.json` in structured attrs mode.
///
/// Starts with the derivation's structured attrs, then adds an `outputs` map
/// mapping output names to their resolved store paths (for input-addressed
/// derivations, these are the final paths).
fn prepare_structured_attrs(
    drv: &BasicDerivation,
    store_dir: &StoreDir,
    output_paths: &[(OutputName, StorePath)],
) -> serde_json::Value {
    let mut json = drv
        .structured_attrs
        .as_ref()
        .map(|sa| serde_json::Value::Object(sa.attrs.clone()))
        .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));

    // Add outputs map: { "out": "/nix/store/...", "dev": "/nix/store/..." }
    let mut outputs_json = serde_json::Map::new();
    for (name, path) in output_paths {
        let full_path = store_dir.to_path().join(path.to_string());
        outputs_json.insert(
            name.to_string(),
            serde_json::Value::String(full_path.to_string_lossy().to_string()),
        );
    }
    json.as_object_mut()
        .unwrap()
        .insert("outputs".into(), serde_json::Value::Object(outputs_json));

    json
}

/// Dispatch to a builtin builder.
///
/// Builtin builders run in-process instead of spawning an external process.
/// They are identified by `builder = "builtin:<name>"` in the derivation.
async fn run_builtin_builder(
    name: &str,
    drv: &BasicDerivation,
    env: &BTreeMap<String, String>,
    output_paths: &[(OutputName, StorePath)],
    store_dir: &StoreDir,
) -> Result<(), BuildError> {
    match name {
        "fetchurl" => {
            crate::builtins::fetchurl::builtin_fetchurl(drv, env, output_paths, store_dir).await
        }
        "buildenv" => {
            crate::builtins::buildenv::builtin_buildenv(drv, env, output_paths, store_dir)
        }
        "unpack-channel" => {
            crate::builtins::unpack_channel::builtin_unpack_channel(
                drv,
                env,
                output_paths,
                store_dir,
            )
            .await
        }
        _ => Err(BuildError::Other(format!(
            "unsupported builtin builder 'builtin:{name}'"
        ))),
    }
}

/// Errors from builder execution.
pub enum BuildError {
    Timeout,
    ExitCode(i32),
    Other(String),
}

/// Execute the builder process, collecting log output.
///
/// Spawns the builder via `NoSandbox` (no isolation) and monitors
/// it for completion, timeouts, and log output.
async fn execute_builder(
    builder: &str,
    args: &[&str],
    env: &BTreeMap<String, String>,
    work_dir: &Path,
    config: &BuildConfig,
    log_sink: &Arc<std::sync::Mutex<dyn Write + Send>>,
) -> Result<(), BuildError> {
    let sandbox = crate::sandbox::NoSandbox::new();
    let child = sandbox
        .spawn(builder, args, env, work_dir)
        .await
        .map_err(|e| BuildError::Other(format!("Failed to spawn builder '{builder}': {e}")))?;

    monitor_child(child, config, log_sink).await
}

/// Monitor a sandbox child process: drain stdout/stderr to a log sink,
/// enforce wall-clock and max-silent-time timeouts, and return the exit
/// status.
///
/// The child must have been spawned with `process_group(0)` so that
/// timeout kills hit the entire process tree.
///
/// Supports both wall-clock timeout and max-silent-time (no log output)
/// timeout. Either triggers a SIGKILL to the process group.
pub(crate) async fn monitor_child(
    mut child: crate::sandbox::SandboxChild,
    config: &BuildConfig,
    log_sink: &Arc<std::sync::Mutex<dyn Write + Send>>,
) -> Result<(), BuildError> {
    let child_pid = child.pid();

    // Drain both stdout and stderr concurrently, writing each line directly
    // to the caller's log sink (typically a file) without buffering in memory.
    // Both streams update last_output on each line for max_silent_time tracking.
    let stdout = child.take_stdout();
    let stderr = child.take_stderr();
    let last_output = Arc::new(std::sync::Mutex::new(tokio::time::Instant::now()));

    let last_out = Arc::clone(&last_output);
    let sink_out = Arc::clone(log_sink);
    let stdout_task = tokio::spawn(async move {
        if let Some(stdout) = stdout {
            let mut reader = tokio::io::BufReader::new(stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                *last_out.lock().unwrap() = tokio::time::Instant::now();
                let mut sink = sink_out.lock().unwrap();
                let _ = writeln!(sink, "{line}");
            }
        }
    });

    let last_err = Arc::clone(&last_output);
    let sink_err = Arc::clone(log_sink);
    let stderr_task = tokio::spawn(async move {
        if let Some(stderr) = stderr {
            let mut reader = tokio::io::BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                *last_err.lock().unwrap() = tokio::time::Instant::now();
                let mut sink = sink_err.lock().unwrap();
                let _ = writeln!(sink, "{line}");
            }
        }
    });

    // Combined wait: wall-clock timeout, max-silent timeout, or process exit
    let status: std::process::ExitStatus = {
        let wall_deadline = config.timeout.map(|d| tokio::time::Instant::now() + d);
        let max_silent = config.max_silent_time;

        loop {
            // Compute next check time:
            // - If wall_deadline is set, that's a hard upper bound
            // - If max_silent is set, check every 50ms whether silence exceeded
            // - Otherwise just await the child
            let sleep_dur = if max_silent.is_some() {
                std::time::Duration::from_millis(50)
            } else if let Some(deadline) = wall_deadline {
                deadline.saturating_duration_since(tokio::time::Instant::now())
            } else {
                // No timeouts configured — just wait for the child
                match child.wait().await {
                    Ok(s) => break s,
                    Err(e) => return Err(BuildError::Other(format!("Wait error: {e}"))),
                }
            };

            tokio::select! {
                result = child.wait() => {
                    match result {
                        Ok(s) => break s,
                        Err(e) => return Err(BuildError::Other(format!("Wait error: {e}"))),
                    }
                }
                _ = tokio::time::sleep(sleep_dur) => {
                    // Check wall-clock timeout
                    if let Some(deadline) = wall_deadline
                        && tokio::time::Instant::now() >= deadline {
                            kill_process_group(child_pid);
                            let _ = child.kill().await;
                            let _ = stdout_task.await;
                            let _ = stderr_task.await;
                            return Err(BuildError::Timeout);
                        }
                    // Check max-silent timeout
                    if let Some(max_silent) = max_silent {
                        let elapsed = last_output.lock().unwrap().elapsed();
                        if elapsed >= max_silent {
                            kill_process_group(child_pid);
                            let _ = child.kill().await;
                            let _ = stdout_task.await;
                            let _ = stderr_task.await;
                            return Err(BuildError::Timeout);
                        }
                    }
                }
            }
        }
    };

    // Wait for drain tasks to flush remaining output to the sink
    let _ = stdout_task.await;
    let _ = stderr_task.await;

    if status.success() {
        Ok(())
    } else {
        Err(BuildError::ExitCode(status.code().unwrap_or(-1)))
    }
}

/// Send SIGKILL to the entire process group rooted at `pid`.
fn kill_process_group(pid: Option<u32>) {
    if let Some(pid) = pid {
        // Negative PID means "kill the process group with PGID == pid"
        // SAFETY: This is a standard POSIX signal operation. The process group
        // was created by us via process_group(0) above.
        #[allow(unsafe_code)]
        unsafe {
            libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
        }
    }
}

/// Single-pass NAR hash computation + reference scanning.
///
/// Streams the path as NAR, feeding each chunk to both a SHA-256 hasher
/// and a [`RefScanSink`]. This mirrors Nix's `TeeSink{refsSink, hashSink}`
/// pattern — one disk read, two consumers.
async fn hash_and_scan(
    path: &Path,
    input_paths: &BTreeSet<StorePath>,
    self_path: &StorePath,
) -> DaemonResult<(NarHash, u64, BTreeSet<StorePath>)> {
    let mut hasher = Context::new(Algorithm::SHA256);
    let mut total_size: u64 = 0;
    let mut ref_sink = RefScanSink::new(input_paths, Some(self_path));

    let mut stream = harmonia_nar::NarByteStream::new(path.to_path_buf());
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| ProtocolError::custom(format!("NAR stream error: {e}")))?;
        hasher.update(&chunk);
        ref_sink.feed(&chunk);
        total_size += chunk.len() as u64;
    }

    let hash = hasher.finish();
    let nar_hash = NarHash::try_from(hash)
        .map_err(|e| ProtocolError::custom(format!("Hash conversion error: {e}")))?;

    Ok((nar_hash, total_size, ref_sink.found_paths()))
}

/// Register built outputs in the database.
///
/// Uses the same `RegisterPathParams` pattern as `add_to_store_nar`.
/// When `repair` is true, existing DB entries are deleted before re-inserting
/// so that hash/size/references are updated to match the rebuilt output.
async fn register_outputs(
    store_dir: &StoreDir,
    db: &Arc<Mutex<harmonia_store_db::StoreDb>>,
    drv_path: &StorePath,
    outputs: &[BuiltOutput],
    repair: bool,
) -> DaemonResult<()> {
    let db = db.clone();
    let store_dir = store_dir.clone();
    let drv_path = drv_path.clone();
    let outputs: Vec<_> = outputs.to_vec();

    tokio::task::spawn_blocking(move || {
        let mut db = db.blocking_lock();
        for output in &outputs {
            let full_path = store_dir
                .to_path()
                .join(output.path.to_string())
                .to_string_lossy()
                .to_string();

            // In repair mode, remove the old entry so we can re-register
            if repair {
                let _ = db.invalidate_path(&full_path);
            }

            let hash_str = format!("{}", output.nar_hash.as_base16());
            let deriver_str = store_dir
                .to_path()
                .join(drv_path.to_string())
                .to_string_lossy()
                .to_string();
            let refs: BTreeSet<String> = output
                .references
                .iter()
                .map(|r| {
                    store_dir
                        .to_path()
                        .join(r.to_string())
                        .to_string_lossy()
                        .to_string()
                })
                .collect();

            let params = harmonia_store_db::RegisterPathParams {
                path: full_path,
                hash: hash_str,
                registration_time: SystemTime::now(),
                deriver: Some(deriver_str),
                nar_size: Some(output.nar_size),
                ultimate: true,
                sigs: None,
                ca: None,
                references: refs,
            };

            db.register_valid_path(&params)
                .map_err(|e| ProtocolError::custom(format!("Database error: {e}")))?;
        }
        Ok(())
    })
    .await
    .map_err(|e| ProtocolError::custom(format!("Task join error: {e}")))?
}

/// Clean up (or preserve) output paths after a failed build.
async fn cleanup_outputs(
    store_dir: &StoreDir,
    output_paths: &[(OutputName, StorePath)],
    keep_failed: bool,
) {
    for (_name, out_path) in output_paths {
        let full_path = store_dir.to_path().join(out_path.to_string());
        if keep_failed && full_path.exists() {
            let failed_path = full_path.with_file_name(format!("{}.failed", out_path));
            let _ = tokio::fs::rename(&full_path, &failed_path).await;
        } else {
            let _ = tokio::fs::remove_dir_all(&full_path).await;
            let _ = tokio::fs::remove_file(&full_path).await;
        }
    }
}

fn now_secs() -> libc::time_t {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as libc::time_t
}

fn make_failure(
    status: FailureStatus,
    error_msg: String,
    start_time: libc::time_t,
    stop_time: libc::time_t,
) -> BuildResult {
    BuildResult {
        inner: BuildResultInner::Failure(BuildResultFailure {
            status,
            error_msg: error_msg.into(),
            is_non_deterministic: status == FailureStatus::NotDeterministic,
        }),
        times_built: 0,
        start_time,
        stop_time,
        cpu_user: None,
        cpu_system: None,
    }
}
