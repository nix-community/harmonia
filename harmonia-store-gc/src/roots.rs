// SPDX-FileCopyrightText: 2026 Jörg Thalheim
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
use harmonia_store_fs::StoreLayout;
use tracing::debug;

use crate::HashSet;
use crate::error::{Error, Result};

/// Find all GC roots by walking the gcroots/profiles directories (plus
/// any `extra_dirs`) and scanning running processes.
///
/// Returned nodes belong to the graph behind `idx`. Runtime-scan
/// candidates that are not in the database are dropped, mirroring Nix's
/// `findRuntimeRoots`.
pub(crate) fn find_roots(
    layout: &StoreLayout,
    extra_dirs: &[PathBuf],
    idx: &BasenameIndex,
) -> Result<Vec<NodeIdx>> {
    let mut roots = HashSet::default();
    let state_dir = layout.state_dir();
    // Store paths are ASCII, so string matching on the prefix is exact.
    let store_prefix = layout.store_dir().to_str().to_string();

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
    let canonical_prefix = Some(layout.real_store_dir().to_string_lossy().into_owned());

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

/// Errors that prove a path does not exist (vs. EACCES/EIO which don't).
fn is_missing(e: &std::io::Error) -> bool {
    use std::io::ErrorKind::*;
    matches!(e.kind(), NotFound | NotADirectory | InvalidFilename)
        || e.raw_os_error() == Some(nix::libc::ELOOP)
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
        Err(e) if is_missing(&e) => {
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
        Err(e) if is_missing(&e) => return Ok(()),
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
    if fs::symlink_metadata(&abs_target2).is_err_and(|e| is_missing(&e)) {
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
    c.is_ascii() && is_store_path_byte(c as u8)
}

fn is_store_path_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.' | b'_' | b'?' | b'=')
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

/// Searches blobs (e.g. environ) for embedded store paths. Scans raw
/// bytes so non-UTF8 data cannot perturb matches, and reuses the SIMD
/// searcher across blobs.
struct BlobScanner<'a> {
    store_prefix: &'a str,
    finder: memchr::memmem::Finder<'static>,
    prefix_len: usize,
}

impl<'a> BlobScanner<'a> {
    fn new(store_prefix: &'a str) -> Self {
        let prefix = format!("{store_prefix}/");
        BlobScanner {
            store_prefix,
            prefix_len: prefix.len(),
            finder: memchr::memmem::Finder::new(prefix.as_bytes()).into_owned(),
        }
    }

    /// Matches are bounded by the store-path byte alphabet, not
    /// arbitrary delimiters. Bare prefixes are rejected by
    /// add_unchecked's basename validation.
    fn scan(&self, blob: &[u8], unchecked: &mut HashSet<String>) {
        let mut search_from = 0;
        while let Some(idx) = self.finder.find(&blob[search_from..]) {
            let abs = search_from + idx;
            let after = abs + self.prefix_len;
            let end = blob[after..]
                .iter()
                .position(|&b| !is_store_path_byte(b))
                .map(|e| after + e)
                .unwrap_or(blob.len());
            // ASCII by construction, so this cannot fail
            if let Ok(m) = std::str::from_utf8(&blob[abs..end]) {
                add_unchecked(self.store_prefix, m, unchecked);
            }
            // end >= after > abs guarantees progress
            search_from = end;
        }
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
    use super::{BlobScanner, add_unchecked};
    use crate::HashSet;
    use rayon::iter::{IntoParallelRefIterator, ParallelIterator};
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

    fn scan_pid(
        pid_dir: &Path,
        store_prefix: &str,
        scanner: &BlobScanner,
        unchecked: &mut HashSet<String>,
    ) {
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

        if let Ok(env_data) = fs::read(pid_dir.join("environ")) {
            scanner.scan(&env_data, unchecked);
        }
    }

    pub fn scan(store_prefix: &str, unchecked: &mut HashSet<String>) {
        let scanner = BlobScanner::new(store_prefix);
        let entries = match fs::read_dir("/proc") {
            Ok(e) => e,
            Err(_) => return,
        };
        let pid_dirs: Vec<std::path::PathBuf> = entries
            .flatten()
            .filter(|e| {
                let name = e.file_name();
                let name = name.to_string_lossy();
                !name.is_empty() && name.chars().all(|c| c.is_ascii_digit())
            })
            .map(|e| e.path())
            .collect();

        // The reads are kernel-CPU bound (seq_file generation), so they
        // parallelize well across pids.
        let merged = pid_dirs
            .par_iter()
            .fold(HashSet::default, |mut acc, pid_dir| {
                scan_pid(pid_dir, store_prefix, &scanner, &mut acc);
                acc
            })
            .reduce(HashSet::default, |mut a, b| {
                a.extend(b);
                a
            });
        unchecked.extend(merged);

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

#[cfg(test)]
mod tests {
    use super::*;
    use harmonia_store_db::{BasenameIndex, GraphOptions, StoreDb, StoreGraph};
    use harmonia_store_path::StoreDir;

    const HASH: &str = "abcdefghijklmnopqrstuvwxyz012345"; // 32 chars

    #[test]
    fn scan_blob_finds_embedded_store_paths() {
        let sp1 = format!("/nix/store/{HASH}-foo");
        let sp2 = format!("/nix/store/{HASH}-bar");
        // Paths embedded mid-blob with trailing junk, and one ending the
        // blob.
        let blob = format!("PATH={sp1}/bin:other LD={sp2}");
        let mut set = HashSet::default();
        BlobScanner::new("/nix/store").scan(blob.as_bytes(), &mut set);
        assert_eq!(set.len(), 2, "{set:?}");
        assert!(set.contains(&sp1));
        assert!(set.contains(&sp2));
    }

    #[test]
    fn scan_blob_ignores_bare_prefix() {
        let mut set = HashSet::default();
        BlobScanner::new("/nix/store").scan(b"x /nix/store/ y /nix/store::", &mut set);
        assert!(set.is_empty(), "{set:?}");
    }

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
        let layout = StoreLayout::new(Path::new("/nix/store"), &state).unwrap();

        // Without the extra dir the symlink is invisible.
        let roots = find_roots(&layout, &[], &idx).unwrap();
        assert!(roots.is_empty(), "{roots:?}");

        // With it, the symlink target becomes a root.
        let roots = find_roots(&layout, &[extra], &idx).unwrap();
        let expected = idx.get(&target).unwrap();
        assert_eq!(roots, vec![expected], "{roots:?}");
    }

    /// An auto root whose target's parent became a regular file (ENOTDIR)
    /// is just as gone as ENOENT and must not abort the whole scan.
    #[test]
    fn indirect_root_through_non_directory_is_stale() {
        let tmp = tempfile::tempdir().unwrap();
        let state = tmp.path().join("state");
        let auto = state.join("gcroots/auto");
        fs::create_dir_all(&auto).unwrap();
        fs::create_dir_all(state.join("profiles")).unwrap();
        let not_a_dir = tmp.path().join("x");
        fs::write(&not_a_dir, b"").unwrap();
        std::os::unix::fs::symlink(not_a_dir.join("result"), auto.join("r1")).unwrap();

        let g = graph(&[&format!("{HASH}-foo")]);
        let idx = BasenameIndex::new(&g);
        let layout = StoreLayout::new(Path::new("/nix/store"), &state).unwrap();
        let roots = find_roots(&layout, &[], &idx).unwrap();
        assert!(roots.is_empty());
        assert!(
            fs::symlink_metadata(auto.join("r1")).is_err(),
            "stale link removed"
        );
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
}
