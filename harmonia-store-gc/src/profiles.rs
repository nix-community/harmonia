// SPDX-FileCopyrightText: 2026 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! Profile generation cleanup (`--delete-old` / `--delete-older-than`).
//!
//! A profile is a symlink like `system` pointing at its current
//! generation link `system-42-link`. Removing old generation links is
//! what allows their store paths to become garbage.

use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use nix::errno::Errno;
use nix::fcntl::{Flock, FlockArg};
use rayon::iter::{IntoParallelRefIterator, ParallelIterator};
use tracing::{info, warn};

use crate::error::{Error, Result};

/// Parse a Nix-style time spec into a point in the past.
///
/// ```
/// use harmonia_store_gc::profiles::parse_older_than;
///
/// assert!(parse_older_than("30d").is_ok());
/// assert!(parse_older_than("4h").is_ok());
/// assert!(parse_older_than("banana").is_err());
/// ```
pub fn parse_older_than(spec: &str) -> Result<SystemTime> {
    let invalid = |reason: &str| Error::TimeSpec {
        spec: spec.to_owned(),
        reason: reason.to_owned(),
    };
    // Take the last *character*. A byte-based split_at would panic on
    // multi-byte input like "30日".
    let Some((idx, unit)) = spec.char_indices().last() else {
        return Err(invalid("expected e.g. '30d'"));
    };
    let num_str = &spec[..idx];
    if num_str.is_empty() {
        return Err(invalid("expected e.g. '30d'"));
    }
    let num: u64 = num_str
        .parse()
        .map_err(|e| invalid(&format!("invalid number: {e}")))?;
    let unit_secs = match unit {
        'h' => 3600,
        'd' => 86400,
        'w' => 7 * 86400,
        'm' => 30 * 86400,
        _ => return Err(invalid(&format!("unknown time unit '{unit}', use h/d/w/m"))),
    };
    let secs = num
        .checked_mul(unit_secs)
        .ok_or_else(|| invalid("out of range"))?;
    SystemTime::now()
        .checked_sub(Duration::from_secs(secs))
        .ok_or_else(|| invalid("out of range"))
}

fn find_generation_links(profile: &Path) -> Result<Vec<(PathBuf, u64)>> {
    let parent = profile.parent().unwrap_or(Path::new("."));
    let profile_name = profile
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let mut gens = Vec::new();
    let entries = match fs::read_dir(parent) {
        Ok(e) => e,
        Err(_) => return Ok(gens),
    };

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if let Some(rest) = name.strip_prefix(&format!("{profile_name}-"))
            && let Some(num_str) = rest.strip_suffix("-link")
            && let Ok(r#gen) = num_str.parse::<u64>()
        {
            gens.push((entry.path(), r#gen));
        }
    }
    gens.sort_by_key(|(_, g)| *g);
    Ok(gens)
}

fn current_generation(profile: &Path) -> Result<Option<u64>> {
    let target = match fs::read_link(profile) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(Error::ReadLink {
                path: profile.to_owned(),
                source,
            });
        }
    };
    let name = target
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let profile_name = profile
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    if let Some(rest) = name.strip_prefix(&format!("{profile_name}-"))
        && let Some(num_str) = rest.strip_suffix("-link")
        && let Ok(r#gen) = num_str.parse::<u64>()
    {
        return Ok(Some(r#gen));
    }
    Ok(None)
}

/// Lock held while mutating a profile's generations, interoperable with
/// Nix's PathLocks protocol: flock(2) on `<profile>.lock`, stale
/// detection via a non-empty file, deletion marker on release.
struct ProfileLock {
    lock_path: PathBuf,
    file: nix::fcntl::Flock<fs::File>,
}

impl ProfileLock {
    fn acquire(profile: &Path) -> Result<ProfileLock> {
        let mut name = profile.file_name().unwrap_or_default().to_os_string();
        name.push(".lock");
        let lock_path = profile.with_file_name(name);
        let lock_err = |source| Error::ProfileLock {
            path: lock_path.clone(),
            source,
        };
        loop {
            let mut file = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .mode(0o600)
                .open(&lock_path)
                .map_err(lock_err)?;
            let file = loop {
                match Flock::lock(file, FlockArg::LockExclusive) {
                    Ok(lock) => break lock,
                    // Flock does not retry EINTR itself.
                    Err((f, Errno::EINTR)) => file = f,
                    Err((_, errno)) => return Err(lock_err(errno.into())),
                }
            };
            // Stale check (Nix protocol): a non-empty lock file was marked
            // and unlinked by its previous holder. Retry on a fresh inode.
            if file.metadata().map_err(lock_err)?.len() != 0 {
                continue;
            }
            return Ok(ProfileLock { lock_path, file });
        }
    }
}

impl Drop for ProfileLock {
    fn drop(&mut self) {
        // Same order as Nix's deleteLockFile: a crash in between must not
        // leave a linked non-empty file, which waiters retry on forever.
        let _ = fs::remove_file(&self.lock_path);
        let _ = self.file.write_all(b"d");
    }
}

fn delete_old_generations(profile: &Path, dry_run: bool) -> Result<()> {
    // Serialize against nix-env/nixos-rebuild generation switches, which
    // take the same lock.
    let _lock = ProfileLock::acquire(profile)?;
    // Fail closed: without a known active generation, "delete all but
    // current" would delete the active system too.
    let Some(current) = current_generation(profile)? else {
        warn!(
            "cannot determine current generation of {}, skipping",
            profile.display()
        );
        return Ok(());
    };
    let gens = find_generation_links(profile)?;

    for (path, r#gen) in &gens {
        if *r#gen == current {
            continue;
        }
        if dry_run {
            info!("would remove: {}", path.display());
        } else {
            info!("removing: {}", path.display());
            if let Err(e) = fs::remove_file(path) {
                warn!("cannot remove {}: {e}", path.display());
            }
        }
    }
    Ok(())
}

fn delete_generations_older_than(profile: &Path, cutoff: SystemTime, dry_run: bool) -> Result<()> {
    let _lock = ProfileLock::acquire(profile)?;
    // Same fail-closed rule as delete_old_generations.
    let Some(current) = current_generation(profile)? else {
        warn!(
            "cannot determine current generation of {}, skipping",
            profile.display()
        );
        return Ok(());
    };
    let gens = find_generation_links(profile)?;

    let mtime_of = |path: &Path| -> Option<SystemTime> {
        let meta = fs::symlink_metadata(path).ok()?;
        Some(meta.modified().unwrap_or(SystemTime::UNIX_EPOCH))
    };

    // Like Nix, keep the newest generation older than the cutoff: it was
    // active at that point in time and stays available for rollback.
    let newest_older = gens
        .iter()
        .rev()
        .find(|(path, _)| mtime_of(path).is_some_and(|t| t < cutoff))
        .map(|(_, g)| *g);

    for (path, r#gen) in &gens {
        if *r#gen == current || Some(*r#gen) == newest_older {
            continue;
        }
        let Some(mtime) = mtime_of(path) else {
            continue;
        };
        if mtime < cutoff {
            if dry_run {
                info!("would remove (old): {}", path.display());
            } else {
                info!("removing (old): {}", path.display());
                if let Err(e) = fs::remove_file(path) {
                    warn!("cannot remove {}: {e}", path.display());
                }
            }
        }
    }
    Ok(())
}

/// Remove old generations of every profile found under `dir`,
/// recursively. With `delete_older_than` set, only generations older
/// than that point go. Without it, everything but the current
/// generation goes.
pub fn remove_old_generations(
    dir: &Path,
    delete_older_than: Option<SystemTime>,
    dry_run: bool,
) -> Result<()> {
    let entries: Vec<_> = match fs::read_dir(dir) {
        Ok(rd) => rd.flatten().collect(),
        Err(_) => return Ok(()),
    };

    let can_write = nix::unistd::access(dir, nix::unistd::AccessFlags::W_OK).is_ok();
    if !can_write {
        warn!(
            "no write permission on {}, skipping its profiles",
            dir.display()
        );
    }

    entries.par_iter().try_for_each(|entry| -> Result<()> {
        let path = entry.path();
        let ft = match fs::symlink_metadata(&path) {
            Ok(m) => m.file_type(),
            Err(_) => return Ok(()),
        };

        if ft.is_symlink() && can_write {
            let link_target = match fs::read_link(&path) {
                Ok(t) => t,
                Err(_) => return Ok(()),
            };
            if link_target.to_string_lossy().contains("link") {
                info!("removing old generations of profile {}", path.display());
                if let Some(cutoff) = delete_older_than {
                    delete_generations_older_than(&path, cutoff, dry_run)?;
                } else {
                    delete_old_generations(&path, dry_run)?;
                }
            }
        } else if ft.is_dir() {
            remove_old_generations(&path, delete_older_than, dry_run)?;
        }
        Ok(())
    })?;

    Ok(())
}

/// Directories scanned for profiles, mirroring `nix-collect-garbage`:
/// the system profiles dir (recursed, so per-user is covered) and the
/// invoking user's XDG state profiles dir. Never the home directory
/// itself — [`remove_old_generations`] recurses, and treating arbitrary
/// `*-N-link` symlinks under `$HOME` as generations would delete user
/// data.
pub fn profile_dirs(state_dir: &Path) -> BTreeSet<PathBuf> {
    let mut dirs = BTreeSet::new();

    dirs.insert(state_dir.join("profiles"));

    let xdg_state = std::env::var("XDG_STATE_HOME")
        .map(PathBuf::from)
        .ok()
        .filter(|p| p.is_absolute())
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".local/state"))
        });
    if let Some(state_home) = xdg_state {
        dirs.insert(state_home.join("nix/profiles"));
    }

    dirs
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use std::time::{Duration, SystemTime};

    fn approx_secs_ago(t: SystemTime, secs: u64) {
        let elapsed = SystemTime::now().duration_since(t).unwrap();
        let want = Duration::from_secs(secs);
        let diff = elapsed.abs_diff(want);
        assert!(
            diff < Duration::from_secs(5),
            "expected ~{secs}s ago, got {elapsed:?}"
        );
    }

    #[test]
    fn parse_older_than_units() {
        approx_secs_ago(parse_older_than("2h").unwrap(), 2 * 3600);
        approx_secs_ago(parse_older_than("3d").unwrap(), 3 * 86400);
        approx_secs_ago(parse_older_than("2w").unwrap(), 2 * 7 * 86400);
        approx_secs_ago(parse_older_than("2m").unwrap(), 2 * 30 * 86400);
    }

    #[test]
    fn parse_older_than_rejects_invalid() {
        assert!(parse_older_than("").is_err());
        assert!(parse_older_than("d").is_err());
        assert!(parse_older_than("5x").is_err());
        assert!(parse_older_than("xd").is_err());
        // Multi-byte unit must error, not panic on a char boundary.
        assert!(parse_older_than("30日").is_err());
        assert!(parse_older_than("日").is_err());
        // Overflowing multiplication must error, not wrap.
        assert!(parse_older_than("99999999999999999999d").is_err());
        assert!(parse_older_than("9999999999999999999d").is_err());
        // Fits in u64 after multiplication but reaches the edge of
        // SystemTime's range. Must not panic.
        let _ = parse_older_than("106751991167300d");
    }

    #[test]
    fn generation_links_and_current() {
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path().join("system");

        // Generation links plus noise that must be ignored.
        for n in [3u64, 1, 2] {
            let link = dir.path().join(format!("system-{n}-link"));
            symlink(format!("/nix/store/fake-{n}"), &link).unwrap();
        }
        symlink("/nix/store/x", dir.path().join("other-1-link")).unwrap();
        symlink("/nix/store/x", dir.path().join("system-foo-link")).unwrap();

        symlink(dir.path().join("system-2-link"), &profile).unwrap();

        let gens = find_generation_links(&profile).unwrap();
        let nums: Vec<u64> = gens.iter().map(|(_, g)| *g).collect();
        assert_eq!(nums, vec![1, 2, 3]);
        assert_eq!(gens[0].0, dir.path().join("system-1-link"));

        assert_eq!(current_generation(&profile).unwrap(), Some(2));
    }

    /// Build a temp profile dir with generations 1..=n and a `system`
    /// symlink pointing at `system-{current}-link`.
    fn setup_profile(n: u64, current: u64) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        for i in 1..=n {
            let link = dir.path().join(format!("system-{i}-link"));
            symlink(format!("/nix/store/fake-{i}"), &link).unwrap();
        }
        let profile = dir.path().join("system");
        symlink(dir.path().join(format!("system-{current}-link")), &profile).unwrap();
        (dir, profile)
    }

    fn existing_gens(dir: &Path) -> Vec<u64> {
        find_generation_links(&dir.join("system"))
            .unwrap()
            .into_iter()
            .map(|(_, g)| g)
            .collect()
    }

    fn set_link_mtime(path: &Path, t: SystemTime) {
        use nix::sys::stat::{UtimensatFlags, utimensat};
        use nix::sys::time::TimeSpec;
        let d = t.duration_since(SystemTime::UNIX_EPOCH).unwrap();
        let ts = TimeSpec::new(d.as_secs() as i64, d.subsec_nanos() as i64);
        utimensat(
            nix::fcntl::AT_FDCWD,
            path,
            &ts,
            &ts,
            UtimensatFlags::NoFollowSymlink,
        )
        .unwrap();
    }

    #[test]
    fn delete_old_generations_keeps_only_current() {
        let (dir, profile) = setup_profile(3, 2);

        // Dry run leaves everything in place.
        delete_old_generations(&profile, true).unwrap();
        assert_eq!(existing_gens(dir.path()), vec![1, 2, 3]);

        delete_old_generations(&profile, false).unwrap();
        assert_eq!(existing_gens(dir.path()), vec![2]);
    }

    #[test]
    fn delete_generations_older_than_cutoff() {
        let (dir, profile) = setup_profile(4, 4);
        let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        // gen i has mtime base + i*100s
        for i in 1u64..=4 {
            set_link_mtime(
                &dir.path().join(format!("system-{i}-link")),
                base + Duration::from_secs(i * 100),
            );
        }

        // Cutoff at gen 2's mtime: only gen 1 is older, and it is kept
        // for rollback (active at the cutoff). Nothing goes.
        let cutoff = base + Duration::from_secs(200);
        delete_generations_older_than(&profile, cutoff, true).unwrap();
        assert_eq!(existing_gens(dir.path()), vec![1, 2, 3, 4]);
        delete_generations_older_than(&profile, cutoff, false).unwrap();
        assert_eq!(existing_gens(dir.path()), vec![1, 2, 3, 4]);

        // Cutoff between gen 3 and 4: gens 1-3 are older, gen 3 was
        // active at the cutoff and is kept. Generations 1 and 2 go.
        let cutoff = base + Duration::from_secs(350);
        delete_generations_older_than(&profile, cutoff, false).unwrap();
        assert_eq!(existing_gens(dir.path()), vec![3, 4]);

        // Cutoff after all gens: the newest older one is the current
        // generation itself, so everything else goes.
        delete_generations_older_than(&profile, base + Duration::from_secs(10_000), false).unwrap();
        assert_eq!(existing_gens(dir.path()), vec![4]);
    }

    #[test]
    fn remove_old_generations_recurses_and_skips_non_link_targets() {
        let outer = tempfile::tempdir().unwrap();
        let nested = outer.path().join("per-user").join("alice");
        fs::create_dir_all(&nested).unwrap();
        for i in 1u64..=3 {
            symlink(
                format!("/nix/store/fake-{i}"),
                nested.join(format!("prof-{i}-link")),
            )
            .unwrap();
        }
        symlink(nested.join("prof-3-link"), nested.join("prof")).unwrap();
        // Symlink whose target does not contain "link" must be left alone.
        symlink("/nix/store/zzz", outer.path().join("plain")).unwrap();

        remove_old_generations(outer.path(), None, false).unwrap();

        let gens: Vec<u64> = find_generation_links(&nested.join("prof"))
            .unwrap()
            .into_iter()
            .map(|(_, g)| g)
            .collect();
        assert_eq!(gens, vec![3]);
        assert!(outer.path().join("plain").symlink_metadata().is_ok());
    }

    #[test]
    fn profile_dirs_includes_state_and_xdg_paths_but_not_home() {
        let state = Path::new("/var/state");
        let dirs = profile_dirs(state);
        assert!(dirs.contains(&state.join("profiles")));
        if let Ok(home) = std::env::var("HOME") {
            // $HOME itself must never be scanned recursively.
            assert!(!dirs.contains(&PathBuf::from(&home)));
            if std::env::var("XDG_STATE_HOME").is_err() {
                assert!(dirs.contains(&PathBuf::from(home).join(".local/state/nix/profiles")));
            }
        }
    }

    #[test]
    fn profile_lock_acquire_release_cleans_up() {
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path().join("system");
        let lock_path = dir.path().join("system.lock");
        {
            let _lock = ProfileLock::acquire(&profile).unwrap();
            assert!(lock_path.exists());
        }
        // Released locks are unlinked (Nix deleteLockFile protocol).
        assert!(!lock_path.exists());
        let _lock = ProfileLock::acquire(&profile).unwrap();
    }

    #[test]
    fn unparseable_current_generation_deletes_nothing() {
        // Profile pointing at a non-generation target: refusing to guess
        // protects the active generation from "delete all but current".
        let (dir, profile) = setup_profile(3, 2);
        fs::remove_file(&profile).unwrap();
        symlink("/nix/store/custom-env", &profile).unwrap();

        delete_old_generations(&profile, false).unwrap();
        assert_eq!(existing_gens(dir.path()), vec![1, 2, 3]);

        delete_generations_older_than(&profile, SystemTime::now(), false).unwrap();
        assert_eq!(existing_gens(dir.path()), vec![1, 2, 3]);
    }

    #[test]
    fn current_generation_none_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(current_generation(&dir.path().join("nope")).unwrap(), None);

        // Profile exists but does not point at a -link target.
        let profile = dir.path().join("system");
        symlink("/nix/store/whatever", &profile).unwrap();
        assert_eq!(current_generation(&profile).unwrap(), None);

        // No matching generation links anywhere.
        assert_eq!(find_generation_links(&profile).unwrap(), vec![]);
    }
}
