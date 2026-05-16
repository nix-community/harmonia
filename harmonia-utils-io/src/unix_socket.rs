// SPDX-License-Identifier: EUPL-1.2 OR MIT

//! Bind and connect AF_UNIX sockets at paths longer than `sockaddr_un.sun_path`.
//! FreeBSD reaches them through bindat and connectat. Linux routes plain bind
//! and connect through `/proc/self/fd/<N>/<base>`, as it's just a  magic symlink
//! the kernel resolves via an open dirfd.

use std::io;
use std::path::Path;
use tokio::net::{UnixListener, UnixStream};

/// The maximum length of `sockaddr_un.sun_path` on the current platform,
/// NUL-inclusive. Paths at or above this length need the dirfd workaround.
#[cfg(target_os = "linux")]
const SUN_PATH_MAX: usize = 108;
#[cfg(not(target_os = "linux"))]
const SUN_PATH_MAX: usize = 104;

/// This is similar to [`tokio::net::UnixListener::bind`], but works for paths
/// whose length reaches or exceeds [`SUN_PATH_MAX`].
pub fn bind_unix_long(path: impl AsRef<Path>) -> io::Result<UnixListener> {
    let path = path.as_ref();
    if path.as_os_str().as_encoded_bytes().len() < SUN_PATH_MAX {
        return UnixListener::bind(path);
    }
    long_path::bind(path)
}

/// This is similar to [`tokio::net::UnixStream::connect`], but works for paths
/// whose length reaches or exceeds [`SUN_PATH_MAX`].
pub async fn connect_unix_long(path: impl AsRef<Path>) -> io::Result<UnixStream> {
    let path = path.as_ref();
    if path.as_os_str().as_encoded_bytes().len() < SUN_PATH_MAX {
        return UnixStream::connect(path).await;
    }
    long_path::connect(path).await
}

#[cfg(target_os = "linux")]
mod long_path {
    use super::{Path, SUN_PATH_MAX, UnixListener, UnixStream, io};

    use std::ffi::OsString;
    use std::os::fd;
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    use std::path::PathBuf;

    use nix::fcntl::{OFlag, open};
    use nix::sys::stat::Mode;

    /// Open the socket's parent directory via [`O_PATH`](`OFlag::O_PATH`) (which only
    /// needs the search permission) and build a `/proc/self/fd/<N>/<basename>` path
    /// the kernel resolves through the open dirfd.
    fn proc_path(path: &Path) -> io::Result<(fd::OwnedFd, PathBuf)> {
        let dir = path
            .parent()
            .ok_or_else(|| io::Error::other("socket path has no parent directory"))?;
        let base = path
            .file_name()
            .ok_or_else(|| io::Error::other("socket path has no file name"))?;

        let dirfd = open(
            dir,
            OFlag::O_PATH | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC,
            Mode::empty(),
        )
        .map_err(io::Error::from)?;

        let mut bytes = format!("/proc/self/fd/{}/", dirfd.as_raw_fd()).into_bytes();
        bytes.extend_from_slice(base.as_bytes());
        if bytes.len() >= SUN_PATH_MAX {
            return Err(io::Error::other(format!(
                "synthesized /proc path is too long ({} bytes)",
                bytes.len()
            )));
        }
        Ok((dirfd, PathBuf::from(OsString::from_vec(bytes))))
    }

    pub fn bind(path: &Path) -> io::Result<UnixListener> {
        // `_dirfd` keeps the directory open across the bind, since
        // /proc/self/fd/<N> resolution needs fd N to still be alive.
        let (_dirfd, proc) = proc_path(path)?;
        UnixListener::bind(&proc)
    }

    pub async fn connect(path: &Path) -> io::Result<UnixStream> {
        let (_dirfd, proc) = proc_path(path)?;
        UnixStream::connect(&proc).await
    }
}

#[cfg(target_os = "freebsd")]
#[allow(unsafe_code)]
mod long_path {
    use super::{Path, UnixListener, UnixStream, io};

    use std::ffi::OsStr;
    use std::os::fd::{AsRawFd, OwnedFd};
    use std::os::unix::net::{UnixListener as StdUnixListener, UnixStream as StdUnixStream};

    use nix::fcntl::{OFlag, open};
    use nix::sys::socket::{
        AddressFamily, Backlog, SockFlag, SockType, SockaddrLike, UnixAddr, listen, socket,
    };
    use nix::sys::stat::Mode;

    // libc exposes CAP_BINDAT and CAP_CONNECTAT constants for these syscalls
    // but not the syscalls themselves, so declare them.
    unsafe extern "C" {
        fn bindat(
            fd: libc::c_int,
            s: libc::c_int,
            name: *const libc::sockaddr,
            namelen: libc::socklen_t,
        ) -> libc::c_int;

        fn connectat(
            fd: libc::c_int,
            s: libc::c_int,
            name: *const libc::sockaddr,
            namelen: libc::socklen_t,
        ) -> libc::c_int;
    }

    fn open_dir(dir: &Path) -> io::Result<OwnedFd> {
        // `O_SEARCH` matches chdir's search-bit semantics.
        let flags = OFlag::O_SEARCH | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC;
        open(dir, flags, Mode::empty()).map_err(io::Error::from)
    }

    fn split_path(path: &Path) -> io::Result<(&Path, &OsStr)> {
        let dir = path
            .parent()
            .ok_or_else(|| io::Error::other("socket path has no parent directory"))?;
        let base = path
            .file_name()
            .ok_or_else(|| io::Error::other("socket path has no file name"))?;
        Ok((dir, base))
    }

    pub fn bind(path: &Path) -> io::Result<UnixListener> {
        let (dir, base) = split_path(path)?;
        let dirfd = open_dir(dir)?;
        let addr = UnixAddr::new(base).map_err(io::Error::from)?;

        let sock = socket(
            AddressFamily::Unix,
            SockType::Stream,
            SockFlag::SOCK_CLOEXEC | SockFlag::SOCK_NONBLOCK,
            None,
        )
        .map_err(io::Error::from)?;

        // SAFETY: bindat is bind(2) plus a leading dirfd. UnixAddr's as_ptr/len uphold them.
        let rc = unsafe {
            bindat(
                dirfd.as_raw_fd(),
                sock.as_raw_fd(),
                SockaddrLike::as_ptr(&addr),
                addr.len(),
            )
        };
        if rc < 0 {
            return Err(io::Error::last_os_error());
        }

        let backlog = Backlog::new(128).map_err(io::Error::from)?;
        listen(&sock, backlog).map_err(io::Error::from)?;

        UnixListener::from_std(StdUnixListener::from(sock))
    }

    pub async fn connect(path: &Path) -> io::Result<UnixStream> {
        // connect blocks until the server accepts, meaning running it on the tokio
        // runtime would stall other tasks. So, we push it to a blocking pool.
        let path = path.to_owned();
        let std_stream = tokio::task::spawn_blocking(move || -> io::Result<StdUnixStream> {
            let (dir, base) = split_path(&path)?;
            let dirfd = open_dir(dir)?;
            let addr = UnixAddr::new(base).map_err(io::Error::from)?;

            let sock = socket(
                AddressFamily::Unix,
                SockType::Stream,
                SockFlag::SOCK_CLOEXEC,
                None,
            )
            .map_err(io::Error::from)?;

            // SAFETY: connectat is connect(2) plus a leading dirfd.
            let rc = unsafe {
                connectat(
                    dirfd.as_raw_fd(),
                    sock.as_raw_fd(),
                    SockaddrLike::as_ptr(&addr),
                    addr.len(),
                )
            };
            if rc < 0 {
                return Err(io::Error::last_os_error());
            }

            let stream = StdUnixStream::from(sock);
            stream.set_nonblocking(true)?;
            Ok(stream)
        })
        .await
        .map_err(|e| io::Error::other(format!("connect task panicked: {e}")))??;
        UnixStream::from_std(std_stream)
    }
}

#[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
mod long_path {
    use super::{Path, UnixListener, UnixStream, io};

    fn unsupported() -> io::Error {
        io::Error::other(
            "AF_UNIX socket path exceeds sun_path and this platform has no \
             dirfd-anchored bind/connect fast path",
        )
    }

    pub fn bind(_path: &Path) -> io::Result<UnixListener> {
        Err(unsupported())
    }

    pub async fn connect(_path: &Path) -> io::Result<UnixStream> {
        Err(unsupported())
    }
}

#[cfg(all(test, any(target_os = "linux", target_os = "freebsd")))]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Build a directory tree under `root` until its absolute path reaches
    /// `target_len` bytes, returning the deepest directory.
    fn build_deep_dir(root: &Path, target_len: usize) -> PathBuf {
        let mut cur = root.to_path_buf();
        while cur.as_os_str().len() < target_len {
            cur.push("deeper-segment-padding");
            fs::create_dir_all(&cur).unwrap();
        }
        cur
    }

    #[tokio::test]
    async fn long_path_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let deep = build_deep_dir(tmp.path(), SUN_PATH_MAX + 50);
        let sock = deep.join("s");
        assert!(sock.as_os_str().len() >= SUN_PATH_MAX);

        let listener = bind_unix_long(&sock).unwrap();

        let server = tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut buf = Vec::new();
            s.read_to_end(&mut buf).await.unwrap();
            buf
        });

        let mut client = connect_unix_long(&sock).await.unwrap();
        client.write_all(b"hello").await.unwrap();
        client.shutdown().await.unwrap();

        assert_eq!(server.await.unwrap(), b"hello");
    }

    #[tokio::test]
    async fn short_path_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let sock = tmp.path().join("s");
        assert!(sock.as_os_str().len() < SUN_PATH_MAX);

        let listener = bind_unix_long(&sock).unwrap();

        let server = tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut buf = Vec::new();
            s.read_to_end(&mut buf).await.unwrap();
            buf
        });

        let mut client = connect_unix_long(&sock).await.unwrap();
        client.write_all(b"world").await.unwrap();
        client.shutdown().await.unwrap();

        assert_eq!(server.await.unwrap(), b"world");
    }
}
