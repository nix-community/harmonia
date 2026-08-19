// SPDX-FileCopyrightText: 2025 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! GC root discovery.
//!
//! A root is anything that keeps a store path alive: symlinks under
//! `gcroots/` and `profiles/`, temp-root files of running processes, and
//! runtime references found by scanning processes (/proc on Linux,
//! libproc on macOS).

use std::fs;
use std::path::{Path, PathBuf};

use harmonia_store_db::{BasenameIndex, NodeIdx};
use tracing::debug;

use crate::HashSet;
use crate::error::{Error, Result};

/// Find all GC roots by walking the gcroots/profiles directories (plus
/// any `extra_dirs`) and scanning running processes.
///
/// Returned nodes belong to the graph behind `idx`. Runtime-scan
/// candidates that are not in the database are dropped, mirroring Nix's
/// `findRuntimeRoots`.
pub fn find_roots(
    state_dir: &Path,
    store_dir: &Path,
    extra_dirs: &[PathBuf],
    idx: &BasenameIndex,
) -> Result<Vec<NodeIdx>> {
    let mut roots = HashSet::default();
    let store_prefix = store_dir.to_string_lossy().to_string();

    // Errors here must abort the GC: silently dropping a roots directory
    // (e.g. EACCES) would let the GC delete live paths.
    let default_dirs = [state_dir.join("gcroots"), state_dir.join("profiles")];
    for dir in default_dirs.iter().chain(extra_dirs.iter()) {
        find_roots_in_dir(dir, &store_prefix, idx, &mut roots).map_err(|source| {
            Error::ScanRoots {
                dir: dir.clone(),
                source: Box::new(source),
            }
        })?;
    }

    // The kernel reports canonical (symlink-resolved) paths for fds and
    // mappings, but the DB stores the logical store path. Scan with both
    // prefixes and normalize back to logical before validating.
    let canonical_prefix = fs::canonicalize(store_dir)
        .ok()
        .map(|p| p.to_string_lossy().into_owned());

    let mut candidates = find_runtime_roots(&store_prefix);
    if let Some(canon) = &canonical_prefix
        && canon != &store_prefix
    {
        for c in find_runtime_roots(canon) {
            if let Some(rest) = c.strip_prefix(canon.as_str()) {
                candidates.insert(format!("{store_prefix}{rest}"));
            }
        }
    }

    for candidate in candidates {
        if let Some(node) = idx.get(&candidate) {
            roots.insert(node);
        }
    }

    Ok(roots.into_iter().collect())
}

fn find_roots_in_dir(
    dir: &Path,
    store_prefix: &str,
    idx: &BasenameIndex,
    roots: &mut HashSet<NodeIdx>,
) -> Result<()> {
    let read_dir_err = |source| Error::ReadDir {
        path: dir.to_owned(),
        source,
    };
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        // A missing roots dir contributes no roots. Anything else (EACCES,
        // EIO) hides an unknown number of roots and must fail the GC.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(read_dir_err(e)),
    };

    for entry in entries {
        let entry = entry.map_err(read_dir_err)?;
        let path = entry.path();
        let meta = match fs::symlink_metadata(&path) {
            Ok(m) => m,
            // Entry removed while scanning: not a root anymore.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(source) => return Err(Error::Stat { path, source }),
        };

        if meta.file_type().is_symlink() {
            let target = match fs::read_link(&path) {
                Ok(t) => t,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(source) => return Err(Error::ReadLink { path, source }),
            };
            let target_str = target.to_string_lossy();

            if is_in_store(store_prefix, &target_str) {
                // Direct root pointing into the store.
                if let Some(sp) = extract_store_path(store_prefix, &target_str)
                    && let Some(node) = idx.get(&sp)
                {
                    roots.insert(node);
                }
            } else {
                resolve_indirect_root(dir, &path, &target, store_prefix, idx, roots)?;
            }
        } else if meta.file_type().is_dir() {
            find_roots_in_dir(&path, store_prefix, idx, roots)?;
        } else if meta.file_type().is_file() {
            // Regular file root (e.g. in auto-roots).
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            let candidate = format!("{store_prefix}/{name}");
            if let Some(node) = idx.get(&candidate) {
                roots.insert(node);
            }
        }
    }
    Ok(())
}

/// Indirect root: symlink -> symlink -> store. Nix's findRoots resolves
/// at most one extra hop. Resolve it with lstat + readlink, never a
/// following stat(): under fs.protected_symlinks, following a user-owned
/// link in a sticky directory like /tmp fails with EACCES, even for root.
fn resolve_indirect_root(
    dir: &Path,
    link: &Path,
    target: &Path,
    store_prefix: &str,
    idx: &BasenameIndex,
    roots: &mut HashSet<NodeIdx>,
) -> Result<()> {
    let abs_target = if target.is_absolute() {
        target.to_path_buf()
    } else {
        dir.join(target)
    };
    let target_meta = match fs::symlink_metadata(&abs_target) {
        Ok(m) => m,
        // First hop gone: the indirect root was removed.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            remove_stale_auto_link(dir, link);
            return Ok(());
        }
        // EACCES/EIO say nothing about the target's existence. Dropping
        // the root could let the GC delete a live path.
        Err(source) => {
            return Err(Error::Stat {
                path: abs_target,
                source,
            });
        }
    };
    if !target_meta.file_type().is_symlink() {
        // Plain file or directory: not an indirect store root.
        return Ok(());
    }
    let target2 = match fs::read_link(&abs_target) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(Error::ReadLink {
                path: abs_target,
                source,
            });
        }
    };
    if let Some(sp) = extract_store_path(store_prefix, &target2.to_string_lossy())
        && let Some(node) = idx.get(&sp)
    {
        roots.insert(node);
        return Ok(());
    }
    // No live root behind the link. Clean dangling auto links, but only
    // when the second hop provably does not exist: EACCES/EIO prove
    // nothing.
    let abs_target2 = if target2.is_absolute() {
        target2
    } else {
        abs_target.parent().unwrap_or(Path::new("/")).join(target2)
    };
    if fs::symlink_metadata(&abs_target2).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound) {
        remove_stale_auto_link(dir, link);
    }
    Ok(())
}

/// Remove a dangling link in gcroots/auto. Component match, not
/// substring: "gcroots/automatic" must not qualify.
fn remove_stale_auto_link(dir: &Path, link: &Path) {
    if dir.ends_with("gcroots/auto") {
        debug!("removing stale link {}", link.display());
        fs::remove_file(link).ok();
    }
}

/// Extract the top-level store path from a potentially deeper path,
/// e.g. `/nix/store/abc...-foo/bin/bar` -> `/nix/store/abc...-foo`.
/// Validates that the basename looks like a store path so `..`, `.links`,
/// and other directory entries are never treated as candidate roots.
fn extract_store_path(store_prefix: &str, full_path: &str) -> Option<String> {
    let rest = full_path.strip_prefix(store_prefix)?.strip_prefix('/')?;
    let name = rest.split('/').next()?;
    if !is_store_path_basename(name) {
        return None;
    }
    Some(format!("{store_prefix}/{name}"))
}

/// True if `name` matches the store-path basename grammar:
/// `<nix32hash>-<name>` where the hash is 32 chars of `[0-9a-z]`.
fn is_store_path_basename(name: &str) -> bool {
    let bytes = name.as_bytes();
    if bytes.len() < 34 || bytes[32] != b'-' {
        return false;
    }
    bytes[..32]
        .iter()
        .all(|&b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && name[33..].chars().all(is_store_path_char)
}

/// True for chars allowed in a Nix store path basename.
/// Mirrors Nix's storePathRegex: `[0-9a-z]+[0-9a-zA-Z+\-._?=]*`.
/// We accept the union since we extract only the first path component.
fn is_store_path_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.' | '_' | '?' | '=')
}

/// True if `path` is inside the store directory (not just a string prefix
/// like `/nix/store-other`). The next char after the prefix must be '/'.
fn is_in_store(store_prefix: &str, path: &str) -> bool {
    path.strip_prefix(store_prefix)
        .is_some_and(|rest| rest.starts_with('/'))
}

/// Add an absolute path as an unchecked candidate root if it lies in the
/// store.
fn add_unchecked(store_prefix: &str, target: &str, unchecked: &mut HashSet<String>) {
    if is_in_store(store_prefix, target)
        && let Some(sp) = extract_store_path(store_prefix, target)
    {
        unchecked.insert(sp);
    }
}

/// Scan a blob (e.g. environ) for embedded store paths using the
/// store-path char alphabet, not arbitrary delimiters.
fn scan_blob_for_store_paths(blob: &str, store_prefix: &str, unchecked: &mut HashSet<String>) {
    let prefix = format!("{store_prefix}/");
    let mut search_from = 0;
    while let Some(idx) = blob[search_from..].find(&prefix) {
        let abs = search_from + idx;
        let after = abs + prefix.len();
        let end = blob[after..]
            .find(|c: char| !is_store_path_char(c))
            .map(|e| after + e)
            .unwrap_or(blob.len());
        // A bare prefix (end == after) is rejected by add_unchecked's
        // basename validation, no need to special-case it.
        add_unchecked(store_prefix, &blob[abs..end], unchecked);
        // end >= after > abs, so progress is guaranteed.
        search_from = end;
    }
}

/// Scan running processes for store paths they reference, mirroring
/// Nix's `findRuntimeRootsUnchecked`. Returned candidate paths are
/// *unchecked*: the caller must validate them against the database.
fn find_runtime_roots(store_prefix: &str) -> HashSet<String> {
    let mut unchecked = HashSet::default();
    runtime_roots::scan(store_prefix, &mut unchecked);
    unchecked
}

#[cfg(target_os = "linux")]
mod runtime_roots {
    use super::{add_unchecked, scan_blob_for_store_paths};
    use crate::HashSet;
    use std::fs;
    use std::path::Path;

    /// Read a /proc symlink, swallowing transient errors (process exited,
    /// no permissions).
    fn read_proc_link(path: &Path, store_prefix: &str, unchecked: &mut HashSet<String>) {
        if let Ok(target) = fs::read_link(path)
            && target.is_absolute()
        {
            add_unchecked(store_prefix, &target.to_string_lossy(), unchecked);
        }
    }

    /// Read a /proc/sys file whose content is a path.
    fn read_file_root(path: &Path, store_prefix: &str, unchecked: &mut HashSet<String>) {
        if let Ok(content) = fs::read_to_string(path) {
            add_unchecked(store_prefix, content.trim(), unchecked);
        }
    }

    pub fn scan(store_prefix: &str, unchecked: &mut HashSet<String>) {
        let entries = match fs::read_dir("/proc") {
            Ok(e) => e,
            Err(_) => return,
        };

        for entry in entries.flatten() {
            let pid = entry.file_name().to_string_lossy().to_string();
            if pid.is_empty() || !pid.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            let pid_dir = entry.path();

            read_proc_link(&pid_dir.join("exe"), store_prefix, unchecked);
            read_proc_link(&pid_dir.join("cwd"), store_prefix, unchecked);

            if let Ok(fds) = fs::read_dir(pid_dir.join("fd")) {
                for fd in fds.flatten() {
                    if !fd.file_name().to_string_lossy().starts_with('.') {
                        read_proc_link(&fd.path(), store_prefix, unchecked);
                    }
                }
            }

            // /proc/<pid>/maps: the 6th whitespace-separated field is the
            // mapped file.
            if let Ok(maps) = fs::read_to_string(pid_dir.join("maps")) {
                for line in maps.lines() {
                    if let Some(file) = line.split_whitespace().nth(5)
                        && file.starts_with('/')
                    {
                        add_unchecked(store_prefix, file, unchecked);
                    }
                }
            }

            if let Ok(env_data) =
                fs::read(pid_dir.join("environ")).map(|d| String::from_utf8_lossy(&d).into_owned())
            {
                scan_blob_for_store_paths(&env_data, store_prefix, unchecked);
            }
        }

        // Kernel helper paths can also pin store entries.
        for f in [
            "/proc/sys/kernel/modprobe",
            "/proc/sys/kernel/fbsplash",
            "/proc/sys/kernel/poweroff_cmd",
        ] {
            read_file_root(Path::new(f), store_prefix, unchecked);
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn read_file_root_trims_and_extracts() {
            let tmp = tempfile::tempdir().unwrap();
            let f = tmp.path().join("modprobe");
            let sp = "/nix/store/00000000000000000000000000000000-modprobe";
            fs::write(&f, format!("{sp}/bin/modprobe\n")).unwrap();
            let mut set = HashSet::default();
            read_file_root(&f, "/nix/store", &mut set);
            assert!(set.contains(sp), "{set:?}");
        }

        #[test]
        fn read_file_root_ignores_non_store_paths() {
            let tmp = tempfile::tempdir().unwrap();
            let f = tmp.path().join("modprobe");
            fs::write(&f, "/sbin/modprobe\n").unwrap();
            let mut set = HashSet::default();
            read_file_root(&f, "/nix/store", &mut set);
            assert!(set.is_empty());
        }
    }
}

/// macOS: libproc syscalls instead of shelling out to lsof.
#[cfg(target_os = "macos")]
mod runtime_roots;

/// Other platforms: no runtime root detection.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod runtime_roots {
    use crate::HashSet;
    pub fn scan(_store_prefix: &str, _unchecked: &mut HashSet<String>) {}
}

/// Find temp roots from the temproots directory.
///
/// Each file is named by the PID that wrote it and contains
/// NUL-terminated store paths. A file whose owning process has died is
/// stale: we can acquire a write lock on it because the owner held one.
/// Stale files are removed and their roots ignored, mirroring Nix's
/// `findTempRoots`.
pub fn find_temp_roots(state_dir: &Path) -> Result<HashSet<String>> {
    let mut roots = HashSet::default();
    let temp_dir = state_dir.join("temproots");

    let entries = match fs::read_dir(&temp_dir) {
        Ok(e) => e,
        Err(_) => return Ok(roots),
    };

    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // Hidden files (e.g. portage's .keep) and non-PID names are not
        // temp root files.
        if name.starts_with('.') || !name.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }

        let path = entry.path();
        let temp_root_err = |source| Error::TempRootFile {
            path: path.clone(),
            source,
        };
        let f = match fs::OpenOptions::new().read(true).write(true).open(&path) {
            Ok(f) => f,
            // Owner exited and the file was cleaned up meanwhile.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            // Anything else (EACCES, EIO) hides live roots. Deleting their
            // targets would yank paths from under a running builder.
            Err(e) => return Err(temp_root_err(e)),
        };

        // The owner holds a write lock while alive. If we can take it,
        // the file is stale.
        if let Ok(mut lock) =
            nix::fcntl::Flock::lock(f, nix::fcntl::FlockArg::LockExclusiveNonblock)
        {
            debug!("removing stale temporary roots file {}", path.display());
            fs::remove_file(&path).ok();
            // Nix protocol: write "d" after unlinking so a client that
            // re-acquires its fd sees the marker and recreates the file
            // instead of writing roots into an unlinked inode.
            use std::io::Write;
            let _ = lock.write_all(b"d");
            continue;
        }

        let contents = fs::read(&path).map_err(temp_root_err)?;

        // Each path is NUL-terminated. A trailing run without a NUL is a
        // partial write from a live builder. Drop it.
        let Some(end) = contents.iter().rposition(|&b| b == 0) else {
            continue;
        };
        for segment in contents[..end].split(|&b| b == 0) {
            if segment.is_empty() {
                continue;
            }
            if let Ok(s) = std::str::from_utf8(segment) {
                roots.insert(s.to_string());
            }
        }
    }

    Ok(roots)
}

#[cfg(test)]
mod tests {
    use super::*;
    use harmonia_store_db::{BasenameIndex, GraphOptions, StoreDb, StoreGraph};
    use harmonia_store_path::StoreDir;

    const HASH: &str = "abcdefghijklmnopqrstuvwxyz012345"; // 32 chars

    /// Load a graph containing the given store path basenames.
    fn graph(basenames: &[&str]) -> StoreGraph {
        let db = StoreDb::open_memory().unwrap();
        for name in basenames {
            db.connection()
                .execute(
                    "INSERT INTO ValidPaths (path, hash, registrationTime) \
                     VALUES (?, 'sha256:x', 1)",
                    [format!("/nix/store/{name}")],
                )
                .unwrap();
        }
        let options = GraphOptions {
            keep_derivations: false,
            keep_outputs: false,
        };
        db.load_graph(&StoreDir::default(), &options).unwrap()
    }

    #[test]
    fn find_roots_scans_extra_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let state = tmp.path().join("state");
        fs::create_dir_all(state.join("gcroots")).unwrap();
        fs::create_dir_all(state.join("profiles")).unwrap();
        let extra = tmp.path().join("extra-roots");
        fs::create_dir_all(&extra).unwrap();

        let target = format!("/nix/store/{HASH}-foo");
        std::os::unix::fs::symlink(&target, extra.join("link")).unwrap();

        let g = graph(&[&format!("{HASH}-foo")]);
        let idx = BasenameIndex::new(&g);
        let store_dir = Path::new("/nix/store");

        // Without the extra dir the symlink is invisible.
        let roots = find_roots(&state, store_dir, &[], &idx).unwrap();
        assert!(roots.is_empty(), "{roots:?}");

        // With it, the symlink target becomes a root.
        let roots = find_roots(&state, store_dir, &[extra], &idx).unwrap();
        let expected = idx.get(&target).unwrap();
        assert_eq!(roots, vec![expected], "{roots:?}");
    }

    #[test]
    fn store_path_basename_grammar() {
        assert!(is_store_path_basename(&format!("{HASH}-foo")));
        assert!(is_store_path_basename(&format!("{HASH}-foo-1.2+x_y?z=")));
        // Too short / missing dash at position 32.
        assert!(!is_store_path_basename("short"));
        assert!(!is_store_path_basename(&format!("{HASH}xfoo")));
        // Uppercase or invalid chars in the hash part.
        assert!(!is_store_path_basename(
            "ABCDEFGHIJKLMNOPQRSTUVWXYZ012345-foo"
        ));
        // Invalid char in the name part: both halves must hold.
        assert!(!is_store_path_basename(&format!("{HASH}-foo bar")));
        // .links and .. must never look like store paths.
        assert!(!is_store_path_basename(".links"));
        assert!(!is_store_path_basename(".."));
    }

    #[test]
    fn is_in_store_requires_separator() {
        assert!(is_in_store("/nix/store", "/nix/store/abc"));
        assert!(!is_in_store("/nix/store", "/nix/store"));
        assert!(!is_in_store("/nix/store", "/nix/store-other/abc"));
        assert!(!is_in_store("/nix/store", "/somewhere/else"));
    }

    #[test]
    fn extract_store_path_takes_first_component() {
        let sp = format!("/nix/store/{HASH}-foo");
        assert_eq!(
            extract_store_path("/nix/store", &format!("{sp}/bin/bar")),
            Some(sp.clone())
        );
        assert_eq!(extract_store_path("/nix/store", &sp), Some(sp));
        assert_eq!(
            extract_store_path("/nix/store", "/nix/store/.links/x"),
            None
        );
        assert_eq!(extract_store_path("/nix/store", "/nix/store"), None);
    }

    #[test]
    fn scan_blob_finds_embedded_store_paths() {
        let sp1 = format!("/nix/store/{HASH}-foo");
        let sp2 = format!("/nix/store/{HASH}-bar");
        // Paths embedded mid-blob with trailing junk, and one ending the
        // blob.
        let blob = format!("PATH={sp1}/bin:other LD={sp2}");
        let mut set = HashSet::default();
        scan_blob_for_store_paths(&blob, "/nix/store", &mut set);
        assert_eq!(set.len(), 2, "{set:?}");
        assert!(set.contains(&sp1));
        assert!(set.contains(&sp2));
    }

    #[test]
    fn scan_blob_ignores_bare_prefix() {
        let mut set = HashSet::default();
        scan_blob_for_store_paths("x /nix/store/ y /nix/store::", "/nix/store", &mut set);
        assert!(set.is_empty(), "{set:?}");
    }

    #[test]
    fn temp_roots_reads_locked_skips_stale_and_junk() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("temproots");
        fs::create_dir_all(&dir).unwrap();

        let sp1 = format!("/nix/store/{HASH}-live1");
        let sp2 = format!("/nix/store/{HASH}-live2");
        // Live file: owner (us) holds the write lock. The trailing segment
        // without a NUL is a partial write and must be dropped.
        let live = dir.join("12345");
        fs::write(&live, format!("{sp1}\0{sp2}\0partial")).unwrap();
        let f = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&live)
            .unwrap();
        let _lock =
            nix::fcntl::Flock::lock(f, nix::fcntl::FlockArg::LockExclusiveNonblock).unwrap();

        // Stale file: no lock held, must be removed and ignored.
        let stale = dir.join("999");
        fs::write(&stale, format!("/nix/store/{HASH}-stale\0")).unwrap();

        // Hidden and non-PID files are not temp roots.
        fs::write(dir.join(".keep"), b"junk").unwrap();
        fs::write(dir.join("notapid"), format!("/nix/store/{HASH}-junk\0")).unwrap();

        // Keep an fd on the stale file to observe the "d" marker.
        let stale_fd = fs::File::open(&stale).unwrap();

        let roots = find_temp_roots(tmp.path()).unwrap();
        assert!(roots.contains(&sp1));
        assert!(roots.contains(&sp2));
        assert_eq!(roots.len(), 2, "{roots:?}");
        assert!(!stale.exists(), "stale temp roots file removed");
        // Nix clients detect deletion by reading back a "d" marker.
        {
            use std::io::{Read, Seek};
            let mut f = stale_fd;
            f.seek(std::io::SeekFrom::Start(0)).unwrap();
            let mut b = [0u8; 1];
            f.read_exact(&mut b).unwrap();
            assert_eq!(&b, b"d", "missing deletion marker");
        }
        assert!(dir.join(".keep").exists());
        assert!(dir.join("notapid").exists());
    }

    #[test]
    fn temp_roots_missing_dir_is_empty() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(find_temp_roots(tmp.path()).unwrap().is_empty());
    }
}
