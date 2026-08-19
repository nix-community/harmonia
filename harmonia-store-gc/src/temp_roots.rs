// SPDX-FileCopyrightText: 2025 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! Temp-root client, protocol-compatible with Nix's `addTempRoot`.
//!
//! A temp root pins a store path for the lifetime of one process, e.g. a
//! build that has not registered its outputs yet. Each process owns a
//! `temproots/<pid>` file holding NUL-terminated store paths. Roots are
//! registered either under a momentary shared `gc.lock`, or handed to a
//! running GC through its `gc-socket`.

use std::fs;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use nix::errno::Errno;
use nix::fcntl::{Flock, FlockArg};

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
/// use harmonia_store_gc::temp_roots::TempRoots;
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
    use std::os::unix::fs::OpenOptionsExt;
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
        use std::os::unix::fs::OpenOptionsExt;

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

        let file = loop {
            // An existing file with our pid must be stale: no two live
            // processes share a pid.
            let _ = fs::remove_file(&path);
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
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
                    ) =>
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
                std::io::ErrorKind::BrokenPipe
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::UnexpectedEof
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

#[cfg(test)]
mod tests {
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
}
