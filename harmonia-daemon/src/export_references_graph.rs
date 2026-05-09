// SPDX-FileCopyrightText: 2026 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! Export references graph support.
//!
//! This module handles the `exportReferencesGraph` derivation attribute:
//! parsing the attribute (both structured and non-structured modes),
//! computing transitive closures of store paths, and writing validity
//! registration files into the build directory.
//!
//! It also provides output reference constraint checking
//! (`allowedReferences`, `disallowedReferences`, `allowedRequisites`,
//! `disallowedRequisites`), which relies on the same transitive closure
//! computation.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use tokio::sync::Mutex;

use harmonia_protocol::daemon::DaemonError as ProtocolError;
use harmonia_protocol::daemon::DaemonResult;
use harmonia_store_core::derivation::BasicDerivation;
use harmonia_store_core::store_path::{StoreDir, StorePath};

use crate::build::BuiltOutput;

/// Parse the `exportReferencesGraph` attribute and write closure info files.
///
/// In non-structured mode, `exportReferencesGraph` is a space-separated list
/// of `filename /nix/store/path` pairs in the env map. In structured-attrs
/// mode, it's a JSON object `{ "filename": ["/nix/store/path", ...] }` in
/// the structured attrs.
///
/// For each (filename, paths) pair, the transitive closure is computed and
/// written in Nix's validity registration format:
/// ```text
/// /nix/store/path
/// <deriver or empty>
/// <number of references>
/// <ref1>
/// <ref2>
/// ...
/// ```
pub(crate) async fn write_export_references_graph(
    store_dir: &StoreDir,
    db: &Arc<Mutex<harmonia_store_db::StoreDb>>,
    drv: &BasicDerivation,
    build_dir: &Path,
) -> DaemonResult<()> {
    // Collect (filename, set of store paths) from the derivation
    let exports = parse_export_references_graph(store_dir, drv)?;
    if exports.is_empty() {
        return Ok(());
    }

    let db = db.clone();
    let build_dir = build_dir.to_path_buf();
    let store_dir = store_dir.clone();

    tokio::task::spawn_blocking(move || {
        let db = db.blocking_lock();
        for (filename, store_paths) in &exports {
            // Compute the transitive closure of all specified paths
            let mut closure = BTreeSet::new();
            for path_str in store_paths {
                if let Ok(sp) = store_dir.parse(path_str) {
                    compute_fs_closure(&db, &store_dir, &sp, &mut closure)?;
                }
            }

            // Write validity registration format
            let content = make_validity_registration(&db, &store_dir, &closure)?;
            let file_path = build_dir.join(filename);
            std::fs::write(&file_path, content).map_err(|e| {
                ProtocolError::custom(format!(
                    "Failed to write exportReferencesGraph file '{}': {e}",
                    file_path.display()
                ))
            })?;
        }
        Ok(())
    })
    .await
    .map_err(|e| ProtocolError::custom(format!("Task join error: {e}")))?
}

/// Parse the `exportReferencesGraph` attribute from a derivation.
///
/// Returns a list of (filename, set of full store path strings).
pub(crate) fn parse_export_references_graph(
    store_dir: &StoreDir,
    drv: &BasicDerivation,
) -> DaemonResult<Vec<(String, Vec<String>)>> {
    let mut result = Vec::new();

    if let Some(ref sa) = drv.structured_attrs {
        // Structured attrs mode: { "filename": ["/nix/store/path", ...] }
        if let Some(erg) = sa.attrs.get("exportReferencesGraph")
            && let Some(obj) = erg.as_object()
        {
            for (filename, paths_json) in obj {
                let paths = flatten_json_to_strings(paths_json).map_err(|e| {
                    ProtocolError::custom(format!(
                        "Invalid exportReferencesGraph value for '{filename}': {e}"
                    ))
                })?;
                result.push((filename.clone(), paths));
            }
        }
    } else {
        // Non-structured mode: space-separated "filename1 path1 filename2 path2 ..."
        if let Some(val) = drv.env.get(b"exportReferencesGraph".as_ref()) {
            let val_str = String::from_utf8_lossy(val);
            let tokens: Vec<&str> = val_str.split_whitespace().collect();
            if !tokens.len().is_multiple_of(2) {
                return Err(ProtocolError::custom(format!(
                    "odd number of tokens in 'exportReferencesGraph': '{val_str}'"
                )));
            }
            for pair in tokens.chunks(2) {
                let filename = pair[0].to_string();
                // The path might be just the store path or a full path
                let path_str = pair[1].to_string();
                let full_path = if path_str.starts_with(store_dir.to_str()) {
                    path_str
                } else {
                    format!("{}/{}", store_dir, path_str.trim_start_matches('/'))
                };
                result.push((filename, vec![full_path]));
            }
        }
    }

    Ok(result)
}

/// Flatten a JSON value (string or array of strings) into a Vec<String>.
pub(crate) fn flatten_json_to_strings(value: &serde_json::Value) -> Result<Vec<String>, String> {
    match value {
        serde_json::Value::String(s) => Ok(vec![s.clone()]),
        serde_json::Value::Array(arr) => {
            let mut result = Vec::new();
            for v in arr {
                result.extend(flatten_json_to_strings(v)?);
            }
            Ok(result)
        }
        _ => Err("value is not a string or array".to_string()),
    }
}

/// Compute the transitive closure of a store path (the path + all references, recursively).
pub(crate) fn compute_fs_closure(
    db: &harmonia_store_db::StoreDb,
    store_dir: &StoreDir,
    path: &StorePath,
    closure: &mut BTreeSet<StorePath>,
) -> DaemonResult<()> {
    if closure.contains(path) {
        return Ok(());
    }
    // Only include paths that are registered in the DB
    if !db
        .is_valid_path(store_dir, path)
        .map_err(|e| ProtocolError::custom(format!("Database error: {e}")))?
    {
        // Path not in DB — skip (matches Nix's behavior for paths that
        // are in the input closure but not registered yet)
        return Ok(());
    }

    closure.insert(path.clone());

    let refs = db
        .query_references(store_dir, path)
        .map_err(|e| ProtocolError::custom(format!("Database error: {e}")))?;
    for ref_path in &refs {
        compute_fs_closure(db, store_dir, ref_path, closure)?;
    }
    Ok(())
}

/// Generate the validity registration format for a set of paths.
///
/// Matches Nix's `makeValidityRegistration(paths, showDerivers=false, showHash=false)`:
/// ```text
/// /nix/store/path1
/// <deriver or empty>
/// <number of refs>
/// <ref1>
/// ...
/// /nix/store/path2
/// ...
/// ```
pub(crate) fn make_validity_registration(
    db: &harmonia_store_db::StoreDb,
    store_dir: &StoreDir,
    paths: &BTreeSet<StorePath>,
) -> DaemonResult<String> {
    let mut s = String::new();
    for path in paths {
        s.push_str(&store_dir.display(path).to_string());
        s.push('\n');

        let info = db
            .query_path_info(store_dir, path)
            .map_err(|e| ProtocolError::custom(format!("Database error: {e}")))?;

        if let Some(info) = info {
            // Deriver (empty string if none — Nix uses showDerivers=false so always empty)
            s.push('\n');

            // Number of references
            s.push_str(&info.info.references.len().to_string());
            s.push('\n');

            // References
            for ref_path in &info.info.references {
                s.push_str(&store_dir.display(ref_path).to_string());
                s.push('\n');
            }
        } else {
            // Path not in DB (shouldn't happen since we filtered in compute_fs_closure)
            s.push('\n');
            s.push_str("0\n");
        }
    }
    Ok(s)
}

/// Check output reference constraints: `allowedReferences`, `disallowedReferences`,
/// `allowedRequisites`, `disallowedRequisites`.
///
/// These are parsed from the derivation's env map (non-structured mode) as
/// space-separated lists of store paths. Returns `Ok(())` if all constraints
/// pass, or `Err(msg)` describing the violation.
pub(crate) async fn check_output_constraints(
    store_dir: &StoreDir,
    db: &Arc<Mutex<harmonia_store_db::StoreDb>>,
    drv: &BasicDerivation,
    built_outputs: &[BuiltOutput],
) -> Result<(), String> {
    let disallowed_refs = parse_path_set_env(drv, b"disallowedReferences");
    let allowed_refs = parse_optional_path_set_env(drv, b"allowedReferences");
    let disallowed_requisites = parse_path_set_env(drv, b"disallowedRequisites");
    let allowed_requisites = parse_optional_path_set_env(drv, b"allowedRequisites");

    // Early return if no constraints are set
    if disallowed_refs.is_empty()
        && allowed_refs.is_none()
        && disallowed_requisites.is_empty()
        && allowed_requisites.is_none()
    {
        return Ok(());
    }

    // Collect all output paths for "self" references (outputs can reference each other)
    let output_paths: BTreeSet<String> = built_outputs
        .iter()
        .map(|o| store_dir.display(&o.path).to_string())
        .collect();

    for output in built_outputs {
        let output_full = store_dir.display(&output.path).to_string();

        // Convert references to full path strings for comparison
        let ref_paths: BTreeSet<String> = output
            .references
            .iter()
            .map(|r| store_dir.display(r).to_string())
            .collect();

        // Check disallowedReferences
        for disallowed in &disallowed_refs {
            if ref_paths.contains(disallowed) {
                return Err(format!(
                    "output '{}' is not allowed to refer to path '{}'",
                    output.path, disallowed
                ));
            }
        }

        // Check allowedReferences
        if let Some(ref allowed) = allowed_refs {
            // Build the full allowed set: explicit list + all output paths + self
            let mut full_allowed: BTreeSet<String> = allowed.clone();
            full_allowed.extend(output_paths.iter().cloned());

            for ref_path in &ref_paths {
                if !full_allowed.contains(ref_path) {
                    return Err(format!(
                        "output '{}' is not allowed to refer to path '{}'",
                        output.path, ref_path
                    ));
                }
            }
        }

        // For transitive checks (requisites), compute the closure
        if !disallowed_requisites.is_empty() || allowed_requisites.is_some() {
            let mut closure = BTreeSet::new();
            // Start with the output's direct references
            for ref_sp in &output.references {
                let db = db.clone();
                let ref_sp = ref_sp.clone();
                let sd = store_dir.clone();
                let mut local_closure = BTreeSet::new();
                tokio::task::spawn_blocking(move || {
                    let db = db.blocking_lock();
                    compute_fs_closure(&db, &sd, &ref_sp, &mut local_closure)
                        .map(|_| local_closure)
                })
                .await
                .map_err(|e| format!("Task join error: {e}"))?
                .map_err(|e| format!("Closure computation error: {e}"))?
                .into_iter()
                .for_each(|p| {
                    closure.insert(p);
                });
            }
            // Also include direct references themselves
            closure.extend(output.references.iter().cloned());

            // Convert closure to full path strings for constraint checking
            let closure_strs: BTreeSet<String> = closure
                .iter()
                .map(|p| store_dir.display(p).to_string())
                .collect();

            // Check disallowedRequisites
            for disallowed in &disallowed_requisites {
                if closure_strs.contains(disallowed) {
                    return Err(format!(
                        "output '{}' is not allowed to refer to path '{}' (via transitive closure)",
                        output.path, disallowed
                    ));
                }
            }

            // Check allowedRequisites
            if let Some(ref allowed) = allowed_requisites {
                let mut full_allowed: BTreeSet<String> = allowed.clone();
                full_allowed.extend(output_paths.iter().cloned());
                full_allowed.insert(output_full.clone());

                for req_path in &closure_strs {
                    if !full_allowed.contains(req_path) {
                        return Err(format!(
                            "output '{}' is not allowed to refer to path '{}' (via transitive closure)",
                            output.path, req_path
                        ));
                    }
                }
            }
        }
    }

    Ok(())
}

/// Parse a space-separated list of store paths from a derivation env var.
pub(crate) fn parse_path_set_env(drv: &BasicDerivation, key: &[u8]) -> BTreeSet<String> {
    drv.env
        .get(key)
        .map(|v| {
            String::from_utf8_lossy(v)
                .split_whitespace()
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

/// Parse an optional space-separated list of store paths from a derivation env var.
/// Returns `None` if the env var is not set (different from empty set).
pub(crate) fn parse_optional_path_set_env(
    drv: &BasicDerivation,
    key: &[u8],
) -> Option<BTreeSet<String>> {
    drv.env.get(key).map(|v| {
        String::from_utf8_lossy(v)
            .split_whitespace()
            .map(String::from)
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use harmonia_store_core::derivation::{DerivationOutput, DerivationT, StructuredAttrs};
    use harmonia_store_core::derived_path::OutputName;
    use harmonia_store_core::store_path::StorePath;
    use harmonia_store_path_info::NarHash;

    /// Helper to register a path using the new API.
    fn register_path(
        db: &mut harmonia_store_db::StoreDb,
        store_dir: &StoreDir,
        path: &StorePath,
        references: &BTreeSet<StorePath>,
    ) {
        let hash_bytes = [0u8; 32];
        let nar_hash = NarHash::new(&hash_bytes);
        let info = harmonia_store_path_info::UnkeyedValidPathInfo {
            deriver: None,
            nar_hash,
            references: references.clone(),
            registration_time: None,
            nar_size: 0,
            ultimate: false,
            signatures: BTreeSet::new(),
            ca: None,
            store_dir: store_dir.clone(),
        };
        db.register_valid_path(store_dir, path, &info).unwrap();
    }

    /// Helper: create a minimal derivation with no outputs.
    fn minimal_drv() -> BasicDerivation {
        let mut outputs = BTreeMap::new();
        outputs.insert(
            OutputName::default(),
            DerivationOutput::InputAddressed(
                StorePath::from_base_path("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-dummy").unwrap(),
            ),
        );
        DerivationT {
            name: "test".parse().unwrap(),
            outputs,
            inputs: BTreeSet::new(),
            platform: "x86_64-linux".into(),
            builder: "/bin/sh".into(),
            args: vec![],
            env: BTreeMap::new(),
            structured_attrs: None,
        }
    }

    #[test]
    fn flatten_json_string() {
        let val = serde_json::Value::String("/nix/store/abc".into());
        let result = flatten_json_to_strings(&val).unwrap();
        assert_eq!(result, vec!["/nix/store/abc"]);
    }

    #[test]
    fn flatten_json_array_of_strings() {
        let val = serde_json::json!(["/nix/store/a", "/nix/store/b"]);
        let result = flatten_json_to_strings(&val).unwrap();
        assert_eq!(result, vec!["/nix/store/a", "/nix/store/b"]);
    }

    #[test]
    fn flatten_json_nested_arrays() {
        let val = serde_json::json!([["/nix/store/a"], "/nix/store/b"]);
        let result = flatten_json_to_strings(&val).unwrap();
        assert_eq!(result, vec!["/nix/store/a", "/nix/store/b"]);
    }

    #[test]
    fn flatten_json_number_fails() {
        let val = serde_json::json!(42);
        assert!(flatten_json_to_strings(&val).is_err());
    }

    #[test]
    fn parse_path_set_env_present() {
        let mut drv = minimal_drv();
        drv.env.insert(
            "disallowedReferences".into(),
            "/nix/store/a /nix/store/b".into(),
        );
        let result = parse_path_set_env(&drv, b"disallowedReferences");
        assert_eq!(result.len(), 2);
        assert!(result.contains("/nix/store/a"));
        assert!(result.contains("/nix/store/b"));
    }

    #[test]
    fn parse_path_set_env_missing() {
        let drv = minimal_drv();
        let result = parse_path_set_env(&drv, b"disallowedReferences");
        assert!(result.is_empty());
    }

    #[test]
    fn parse_optional_path_set_env_present_empty() {
        let mut drv = minimal_drv();
        drv.env.insert("allowedReferences".into(), "".into());
        let result = parse_optional_path_set_env(&drv, b"allowedReferences");
        assert!(result.is_some());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn parse_optional_path_set_env_missing() {
        let drv = minimal_drv();
        let result = parse_optional_path_set_env(&drv, b"allowedReferences");
        assert!(result.is_none());
    }

    #[test]
    fn parse_erg_non_structured_single_pair() {
        let store_dir = StoreDir::default();
        let mut drv = minimal_drv();
        let dep_full = format!("{}/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-dep", store_dir);
        drv.env.insert(
            "exportReferencesGraph".into(),
            format!("graph {dep_full}").into(),
        );

        let result = parse_export_references_graph(&store_dir, &drv).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].0, "graph");
        assert_eq!(result[0].1, vec![dep_full]);
    }

    #[test]
    fn parse_erg_non_structured_multiple_pairs() {
        let store_dir = StoreDir::default();
        let mut drv = minimal_drv();
        let dep_a = format!("{}/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-a", store_dir);
        let dep_b = format!("{}/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-b", store_dir);
        drv.env.insert(
            "exportReferencesGraph".into(),
            format!("file1 {dep_a} file2 {dep_b}").into(),
        );

        let result = parse_export_references_graph(&store_dir, &drv).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].0, "file1");
        assert_eq!(result[0].1, vec![dep_a]);
        assert_eq!(result[1].0, "file2");
        assert_eq!(result[1].1, vec![dep_b]);
    }

    #[test]
    fn parse_erg_non_structured_odd_tokens_fails() {
        let store_dir = StoreDir::default();
        let mut drv = minimal_drv();
        drv.env.insert(
            "exportReferencesGraph".into(),
            "graph /nix/store/a extra".into(),
        );

        assert!(parse_export_references_graph(&store_dir, &drv).is_err());
    }

    #[test]
    fn parse_erg_structured_attrs() {
        let store_dir = StoreDir::default();
        let mut drv = minimal_drv();
        let mut attrs = serde_json::Map::new();
        attrs.insert(
            "exportReferencesGraph".into(),
            serde_json::json!({
                "graph": ["/nix/store/aaaa-dep"]
            }),
        );
        drv.structured_attrs = Some(StructuredAttrs { attrs });

        let result = parse_export_references_graph(&store_dir, &drv).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].0, "graph");
        assert_eq!(result[0].1, vec!["/nix/store/aaaa-dep"]);
    }

    #[test]
    fn parse_erg_empty() {
        let store_dir = StoreDir::default();
        let drv = minimal_drv();
        let result = parse_export_references_graph(&store_dir, &drv).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn make_validity_registration_empty() {
        let db = harmonia_store_db::StoreDb::open_memory().unwrap();
        let sd = StoreDir::default();

        let paths = BTreeSet::new();
        let result = make_validity_registration(&db, &sd, &paths).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn make_validity_registration_single_path() {
        let mut db = harmonia_store_db::StoreDb::open_memory().unwrap();
        let sd = StoreDir::default();

        let sp = StorePath::from_base_path("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-pkg").unwrap();
        register_path(&mut db, &sd, &sp, &BTreeSet::new());

        let mut paths = BTreeSet::new();
        paths.insert(sp.clone());

        let result = make_validity_registration(&db, &sd, &paths).unwrap();
        let lines: Vec<&str> = result.lines().collect();
        let full = sd.display(&sp).to_string();
        assert_eq!(lines[0], full);
        assert_eq!(lines[1], ""); // empty deriver
        assert_eq!(lines[2], "0"); // 0 references
    }

    #[test]
    fn make_validity_registration_with_references() {
        let mut db = harmonia_store_db::StoreDb::open_memory().unwrap();
        let sd = StoreDir::default();

        let dep_sp = StorePath::from_base_path("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-dep").unwrap();
        let pkg_sp = StorePath::from_base_path("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-pkg").unwrap();

        register_path(&mut db, &sd, &dep_sp, &BTreeSet::new());

        let mut refs = BTreeSet::new();
        refs.insert(dep_sp.clone());
        register_path(&mut db, &sd, &pkg_sp, &refs);

        let mut paths = BTreeSet::new();
        paths.insert(pkg_sp.clone());
        paths.insert(dep_sp.clone());

        let result = make_validity_registration(&db, &sd, &paths).unwrap();

        let dep_full = sd.display(&dep_sp).to_string();
        let pkg_full = sd.display(&pkg_sp).to_string();

        // Both paths should appear (BTreeSet is sorted)
        assert!(result.contains(&pkg_full));
        assert!(result.contains(&dep_full));

        // pkg_path should list dep_path as a reference
        let lines: Vec<&str> = result.lines().collect();
        // First entry is pkg_path (alphabetical: 'a' < 'b')
        assert_eq!(lines[0], pkg_full);
        assert_eq!(lines[1], ""); // empty deriver
        assert_eq!(lines[2], "1"); // 1 reference
        assert_eq!(lines[3], dep_full);
    }

    #[test]
    fn compute_fs_closure_single_path() {
        let mut db = harmonia_store_db::StoreDb::open_memory().unwrap();
        let sd = StoreDir::default();

        let sp = StorePath::from_base_path("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-pkg").unwrap();
        register_path(&mut db, &sd, &sp, &BTreeSet::new());

        let mut closure = BTreeSet::new();
        compute_fs_closure(&db, &sd, &sp, &mut closure).unwrap();
        assert_eq!(closure.len(), 1);
        assert!(closure.contains(&sp));
    }

    #[test]
    fn compute_fs_closure_transitive() {
        let mut db = harmonia_store_db::StoreDb::open_memory().unwrap();
        let sd = StoreDir::default();

        let leaf = StorePath::from_base_path("cccccccccccccccccccccccccccccccc-leaf").unwrap();
        let mid = StorePath::from_base_path("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-mid").unwrap();
        let root = StorePath::from_base_path("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-root").unwrap();

        register_path(&mut db, &sd, &leaf, &BTreeSet::new());

        let mut mid_refs = BTreeSet::new();
        mid_refs.insert(leaf.clone());
        register_path(&mut db, &sd, &mid, &mid_refs);

        let mut root_refs = BTreeSet::new();
        root_refs.insert(mid.clone());
        register_path(&mut db, &sd, &root, &root_refs);

        let mut closure = BTreeSet::new();
        compute_fs_closure(&db, &sd, &root, &mut closure).unwrap();
        assert_eq!(closure.len(), 3);
        assert!(closure.contains(&root));
        assert!(closure.contains(&mid));
        assert!(closure.contains(&leaf));
    }

    #[test]
    fn compute_fs_closure_skips_unregistered() {
        let db = harmonia_store_db::StoreDb::open_memory().unwrap();
        let sd = StoreDir::default();

        let sp = StorePath::from_base_path("xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx-missing").unwrap();
        let mut closure = BTreeSet::new();
        compute_fs_closure(&db, &sd, &sp, &mut closure).unwrap();
        assert!(closure.is_empty());
    }

    #[test]
    fn compute_fs_closure_handles_cycles() {
        let mut db = harmonia_store_db::StoreDb::open_memory().unwrap();
        let sd = StoreDir::default();

        // Create a self-referencing path (common in glibc etc.)
        let sp = StorePath::from_base_path("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-self").unwrap();
        let mut self_refs = BTreeSet::new();
        self_refs.insert(sp.clone());
        register_path(&mut db, &sd, &sp, &self_refs);

        let mut closure = BTreeSet::new();
        compute_fs_closure(&db, &sd, &sp, &mut closure).unwrap();
        assert_eq!(closure.len(), 1);
        assert!(closure.contains(&sp));
    }
}
