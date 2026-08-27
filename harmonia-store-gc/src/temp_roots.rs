// SPDX-FileCopyrightText: 2026 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! Temp-root client, protocol-compatible with Nix's `addTempRoot`.
//!
//! A temp root pins a store path for the lifetime of one process, e.g. a
//! build that has not registered its outputs yet. Each process owns a
//! `temproots/<pid>` file holding NUL-terminated store paths. Roots are
//! registered either under a momentary shared `gc.lock`, or handed to a
//! running GC through its `gc-socket`.

use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use nix::errno::Errno;
use nix::fcntl::{Flock, FlockArg};

use tracing::debug;

use crate::error::{Error, Result};

/// `Flock::lock` with retry on EINTR, which the nix crate does not do
/// itself. A signal (e.g. SIGCHLD in a build supervisor) must not fail
/// root registration.
fn flock_retry(mut f: fs::File, arg: FlockArg) -> std::io::Result<Flock<fs::File>> {
    loop {
        match Flock::lock(f, arg) {
            Ok(lock) => return Ok(lock),
            Err((file, Errno::EINTR)) => f = file,
            Err((_, errno)) => return Err(errno.into()),
        }
    }
}

/// This process's temp-root registration handle.
///
/// While the value lives, every path passed to [`TempRoots::add`] is
/// protected from garbage collection. Dropping it releases all of them.
///
/// ```no_run
/// # fn main() -> harmonia_store_gc::Result<()> {
/// use std::path::Path;
/// use harmonia_store_gc::TempRoots;
///
/// let mut roots = TempRoots::create(Path::new("/nix/var/nix"))?;
/// roots.add("/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-example")?;
/// # Ok(())
/// # }
/// ```
pub struct TempRoots {
    /// `temproots/<pid>`. We hold the exclusive flock for our lifetime so
    /// a concurrent GC treats the file as live.
    file: Flock<fs::File>,
    /// `gc.lock` fd, locked shared only for the duration of each `add`.
    /// `None` after an unlock failure. Reopened lazily.
    gc_lock: Option<fs::File>,
    gc_lock_path: PathBuf,
    socket_path: PathBuf,
    socket: Option<UnixStream>,
}

fn open_gc_lock(path: &Path) -> Result<fs::File> {
    fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)
        .map_err(|source| Error::GcLock {
            path: path.to_owned(),
            source,
        })
}

impl TempRoots {
    /// Create (or take over) this process's temp roots file, mirroring
    /// Nix's `createTempRootsFile`.
    pub fn create(state_dir: &Path) -> Result<TempRoots> {
        let dir = state_dir.join("temproots");
        fs::create_dir_all(&dir).map_err(|source| Error::TempRootFile {
            path: dir.clone(),
            source,
        })?;
        let path = dir.join(std::process::id().to_string());
        let temp_root_err = |source| Error::TempRootFile {
            path: path.clone(),
            source,
        };

        let mut attempts = 0;
        let file = loop {
            // An existing file with our pid must be stale: no two live
            // processes share a pid.
            if let Err(e) = fs::remove_file(&path)
                && e.kind() != ErrorKind::NotFound
                && attempts > 0
            {
                return Err(temp_root_err(e));
            }
            attempts += 1;
            if attempts > 10 {
                return Err(temp_root_err(std::io::Error::other(
                    "temp roots file keeps reappearing with a deletion marker",
                )));
            }
            let f = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .mode(0o600)
                .open(&path)
                .map_err(temp_root_err)?;
            let locked = flock_retry(f, FlockArg::LockExclusive).map_err(temp_root_err)?;
            // Non-empty means the GC wrote its "d" marker and unlinked the
            // file before we got the lock. Retry on a fresh inode.
            if locked.metadata().map_err(temp_root_err)?.len() == 0 {
                break locked;
            }
        };

        let gc_lock_path = state_dir.join("gc.lock");
        let gc_lock = open_gc_lock(&gc_lock_path)?;

        Ok(TempRoots {
            file,
            gc_lock: Some(gc_lock),
            gc_lock_path,
            socket_path: state_dir.join("gc-socket/socket"),
            socket: None,
        })
    }

    /// Register a full store path as a temp root. After this returns, no
    /// GC will delete the path while this process lives.
    pub fn add(&mut self, store_path: &str) -> Result<()> {
        loop {
            // A shared gc.lock means no GC is running. Hold it across the
            // write so none starts mid-register.
            if let Some(guard) = self.try_shared_gc_lock()? {
                let res = self.write_root(store_path);
                // Unlock failure drops the fd. Reopened on the next add.
                self.gc_lock = guard.unlock().ok();
                return res;
            }
            // GC running: hand the root to it over the gc-socket. The
            // socket may vanish at any time (GC finished). Restart then.
            match self.notify_gc(store_path)? {
                true => return self.write_root(store_path),
                false => continue,
            }
        }
    }

    /// Take the shared gc.lock without blocking. `Ok(None)` means a GC
    /// holds it exclusively.
    fn try_shared_gc_lock(&mut self) -> Result<Option<Flock<fs::File>>> {
        let file = match self.gc_lock.take() {
            Some(f) => f,
            None => open_gc_lock(&self.gc_lock_path)?,
        };
        match Flock::lock(file, FlockArg::LockSharedNonblock) {
            Ok(guard) => Ok(Some(guard)),
            Err((file, Errno::EWOULDBLOCK)) => {
                self.gc_lock = Some(file);
                Ok(None)
            }
            Err((_, errno)) => Err(Error::GcLock {
                path: self.gc_lock_path.clone(),
                source: errno.into(),
            }),
        }
    }

    fn write_root(&mut self, store_path: &str) -> Result<()> {
        self.file
            .write_all(format!("{store_path}\0").as_bytes())
            .map_err(|source| Error::TempRootWrite { source })
    }

    /// Send the root to the running GC. `Ok(false)` means the GC went
    /// away and the caller should restart with the lock.
    fn notify_gc(&mut self, store_path: &str) -> Result<bool> {
        if self.socket.is_none() {
            match UnixStream::connect(&self.socket_path) {
                Ok(s) => self.socket = Some(s),
                Err(e)
                    if matches!(e.kind(), ErrorKind::ConnectionRefused | ErrorKind::NotFound) =>
                {
                    // GC exited or has not created the socket yet.
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    return Ok(false);
                }
                Err(source) => {
                    return Err(Error::GcSocketClient {
                        path: self.socket_path.clone(),
                        source,
                    });
                }
            }
        }
        let sock = self.socket.as_mut().expect("socket set above");
        let gone = |e: &std::io::Error| {
            matches!(
                e.kind(),
                ErrorKind::BrokenPipe | ErrorKind::ConnectionReset | ErrorKind::UnexpectedEof
            )
        };
        let io = (|| -> std::io::Result<()> {
            sock.write_all(format!("{store_path}\n").as_bytes())?;
            let mut ack = [0u8; 1];
            sock.read_exact(&mut ack)?;
            if ack != *b"1" {
                return Err(std::io::Error::other("unexpected gc-socket ack"));
            }
            Ok(())
        })();
        match io {
            Ok(()) => Ok(true),
            Err(e) if gone(&e) => {
                self.socket = None;
                Ok(false)
            }
            Err(source) => Err(Error::GcSocketClient {
                path: self.socket_path.clone(),
                source,
            }),
        }
    }
}

/// Find temp roots from the temproots directory.
///
/// Each file is named by the PID that wrote it and contains
/// NUL-terminated store paths. A file whose owning process has died is
/// stale: we can acquire a write lock on it because the owner held one.
/// Stale files are removed and their roots ignored, mirroring Nix's
/// `findTempRoots`.
pub(crate) fn find_temp_roots(state_dir: &Path) -> Result<std::collections::HashSet<String>> {
    let mut roots = std::collections::HashSet::new();
    let temp_dir = state_dir.join("temproots");

    let read_dir_err = |source| Error::ReadDir {
        path: temp_dir.clone(),
        source,
    };
    let entries = match fs::read_dir(&temp_dir) {
        Ok(e) => e,
        // A missing temproots dir means no registered builds. Anything
        // else (EACCES, EIO) hides live roots and must fail the GC.
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(roots),
        Err(e) => return Err(read_dir_err(e)),
    };

    for entry in entries {
        let entry = entry.map_err(read_dir_err)?;
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
            Err(e) if e.kind() == ErrorKind::NotFound => continue,
            // Anything else (EACCES, EIO) hides live roots. Deleting their
            // targets would yank paths from under a running builder.
            Err(e) => return Err(temp_root_err(e)),
        };

        // The owner holds a write lock while alive. If we can take it,
        // the file is stale.
        let mut file = match Flock::lock(f, FlockArg::LockExclusiveNonblock) {
            Ok(mut lock) => {
                debug!("removing stale temporary roots file {}", path.display());
                fs::remove_file(&path).ok();
                // Nix protocol: write "d" after unlinking so a client that
                // re-acquires its fd sees the marker and recreates the file
                // instead of writing roots into an unlinked inode.
                let _ = lock.write_all(b"d");
                continue;
            }
            // Reading via a fresh open by path would race with the owner
            // exiting and unlinking.
            Err((file, _)) => file,
        };
        let mut contents = Vec::new();
        file.read_to_end(&mut contents).map_err(temp_root_err)?;

        // Each path is NUL-terminated. A trailing run without a NUL is a
        // partial write from a live builder. Drop it.
        let Some(end) = contents.iter().rposition(|&b| b == 0) else {
            continue;
        };
        for segment in contents[..end].split(|&b| b == 0) {
            if segment.is_empty() {
                continue;
            }
            if let Ok(s) = str::from_utf8(segment) {
                roots.insert(s.to_string());
            }
        }
    }

    Ok(roots)
}

#[cfg(test)]
mod tests {
    const HASH: &str = "abcdefghijklmnopqrstuvwxyz012345"; // 32 chars
    use super::*;

    #[test]
    fn add_writes_nul_terminated_roots() {
        let tmp = tempfile::tempdir().unwrap();
        let mut tr = TempRoots::create(tmp.path()).unwrap();
        tr.add("/nix/store/aaa-x").unwrap();
        tr.add("/nix/store/bbb-y").unwrap();

        let path = tmp
            .path()
            .join("temproots")
            .join(std::process::id().to_string());
        let data = fs::read(&path).unwrap();
        assert_eq!(data, b"/nix/store/aaa-x\0/nix/store/bbb-y\0");
        // We hold the write lock, so the file reads as live to a GC.
        let f = fs::File::open(&path).unwrap();
        assert!(Flock::lock(f, FlockArg::LockExclusiveNonblock).is_err());
    }

    #[test]
    fn add_uses_gc_socket_when_gc_holds_the_lock() {
        use std::io::BufRead;
        use std::os::unix::net::UnixListener;

        let tmp = tempfile::tempdir().unwrap();

        // Simulate a running GC: exclusive gc.lock + listening socket.
        let gc_lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(tmp.path().join("gc.lock"))
            .unwrap();
        let _gc_lock = Flock::lock(gc_lock, FlockArg::LockExclusive).unwrap();
        fs::create_dir_all(tmp.path().join("gc-socket")).unwrap();
        let listener = UnixListener::bind(tmp.path().join("gc-socket/socket")).unwrap();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut writer = stream.try_clone().unwrap();
            let mut line = String::new();
            std::io::BufReader::new(stream)
                .read_line(&mut line)
                .unwrap();
            writer.write_all(b"1").unwrap();
            line
        });

        let mut tr = TempRoots::create(tmp.path()).unwrap();
        tr.add("/nix/store/ccc-z").unwrap();

        assert_eq!(server.join().unwrap(), "/nix/store/ccc-z\n");
        let path = tmp
            .path()
            .join("temproots")
            .join(std::process::id().to_string());
        assert_eq!(fs::read(&path).unwrap(), b"/nix/store/ccc-z\0");
    }

    #[test]
    fn find_temp_roots_missing_dir_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let roots = find_temp_roots(&dir.path().join("no-such-state")).unwrap();
        assert!(roots.is_empty());
    }

    #[test]
    fn find_temp_roots_unreadable_dir_is_an_error() {
        use std::os::unix::fs::PermissionsExt;
        if nix::unistd::Uid::effective().is_root() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let temp_dir = dir.path().join("temproots");
        fs::create_dir(&temp_dir).unwrap();
        fs::set_permissions(&temp_dir, fs::Permissions::from_mode(0o000)).unwrap();
        let err = find_temp_roots(dir.path()).unwrap_err();
        fs::set_permissions(&temp_dir, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(err, Error::ReadDir { .. }));
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
        let _lock = Flock::lock(f, FlockArg::LockExclusiveNonblock).unwrap();

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
}
