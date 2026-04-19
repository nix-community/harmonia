use std::cmp::Ordering;
use std::ffi::OsStr;
use std::fs::read_link;
use std::future::Future as _;
use std::os::unix::ffi::OsStrExt;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::task::{Context, Poll, ready};
use std::{collections::VecDeque, io};

use bstr::{ByteSlice as _, ByteVec as _};
use bytes::Bytes;
use futures_core::Stream;
use tokio::io::AsyncRead;
use tokio::task::{JoinHandle, spawn_blocking};
use tracing::debug;
use walkdir::{DirEntry, IntoIter};

use super::{CASE_HACK_SUFFIX, NarEvent};

pub struct DumpOptions {
    use_case_hack: bool,
}

impl DumpOptions {
    pub fn new() -> Self {
        #[cfg(target_os = "macos")]
        let use_case_hack = true;
        #[cfg(not(target_os = "macos"))]
        let use_case_hack = false;
        Self { use_case_hack }
    }

    pub fn use_case_hack(mut self, use_case_hack: bool) -> Self {
        self.use_case_hack = use_case_hack;
        self
    }

    pub fn dump<P: Into<PathBuf>>(self, path: P) -> NarDumper {
        let root = path.into();
        let mut walker = walkdir::WalkDir::new(&root)
            .follow_links(false)
            .follow_root_links(false);
        walker = if self.use_case_hack {
            walker.sort_by(sort_case_hack)
        } else {
            walker.sort_by(|a, b| fast_file_name(a.path()).cmp(fast_file_name(b.path())))
        };
        let walker = walker.into_iter();
        NarDumper {
            state: State::Idle(Some((VecDeque::with_capacity(CHUNK_SIZE), walker, true))),
            next: None,
            level: 0,
            use_case_hack: self.use_case_hack,
        }
    }
}

impl Default for DumpOptions {
    fn default() -> Self {
        Self::new()
    }
}

pub fn dump<P: Into<PathBuf>>(path: P) -> NarDumper {
    DumpOptions::new().dump(path)
}

/// Return the final path segment as raw bytes.
///
/// Uses a byte-level search instead of `Path::file_name`, which performs a
/// full `Components` parse on every call. This is safe because walkdir builds
/// entry paths as `parent.join(file_name)`, so on Unix the segment after the
/// last `/` is exactly the file name with no `.`/`..` to normalise.
#[cfg(unix)]
fn fast_file_name(p: &Path) -> &[u8] {
    let b = p.as_os_str().as_bytes();
    match b.rfind_byte(b'/') {
        Some(i) => &b[i + 1..],
        None => b,
    }
}

fn sort_case_hack(left: &DirEntry, right: &DirEntry) -> Ordering {
    let left_file_name = left.file_name();
    let right_file_name = right.file_name();
    remove_case_hack_osstr(left_file_name)
        .unwrap_or(left_file_name)
        .cmp(remove_case_hack_osstr(right_file_name).unwrap_or(right_file_name))
}

fn remove_case_hack_osstr(name: &OsStr) -> Option<&OsStr> {
    if let Some(n) = <[u8]>::from_os_str(name)
        && let Some(pos) = n.rfind(CASE_HACK_SUFFIX)
    {
        return Some(OsStr::from_bytes(&n[..pos]));
    }
    None
}

fn remove_case_hack(name: &mut Bytes) {
    if let Some(pos) = name.rfind(CASE_HACK_SUFFIX) {
        debug!("removing case hack suffix from '{:?}'", name);
        name.truncate(pos);
    }
}

use super::mmap::MappedFile;

/// Files up to this size are read into a heap buffer; larger files are
/// memory-mapped so streaming a multi-gigabyte store path stays bounded.
const SMALL_FILE_THRESHOLD: u64 = 256 * 1024; // 256 KiB

/// Load a file as a single `Bytes` value without intermediate copies:
/// small files become `Bytes::from(Vec)`, large files become a refcounted
/// view over the mmap via `Bytes::from_owner`.
fn load_file_bytes(path: &Path, size: u64) -> io::Result<Bytes> {
    if size <= SMALL_FILE_THRESHOLD {
        Ok(Bytes::from(std::fs::read(path)?))
    } else {
        Ok(Bytes::from_owner(MappedFile::open(path, size)?))
    }
}

/// File contents for a [`NarEvent::File`], already loaded into memory (or
/// memory-mapped) by the same blocking task that walked the directory.
///
/// The data is fetched eagerly so the async consumer never has to round-trip
/// to the blocking pool per file; neither representation holds an open file
/// descriptor, so at most [`CHUNK_SIZE`] entries worth of small-file buffers
/// plus mappings are resident at a time.
pub struct DumpedFile {
    data: Bytes,
}

impl DumpedFile {
    fn new(data: Bytes) -> Self {
        Self { data }
    }

    /// Take the file content as a zero-copy [`Bytes`].
    pub fn into_bytes(self) -> Bytes {
        self.data
    }
}

impl AsyncRead for DumpedFile {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let n = self.data.len().min(buf.remaining());
        buf.put_slice(&self.data[..n]);
        // `Bytes::advance` on an owned/mmap-backed buffer just bumps an offset.
        bytes::Buf::advance(&mut self.data, n);
        Poll::Ready(Ok(()))
    }
}

enum Entry {
    File {
        depth: usize,
        path: PathBuf,
        size: u64,
        executable: bool,
        data: Bytes,
    },
    Symlink {
        depth: usize,
        path: PathBuf,
        target: PathBuf,
    },
    Directory {
        depth: usize,
        path: PathBuf,
    },
}

impl Entry {
    fn path(&self) -> &Path {
        match self {
            Entry::File { path, .. } => path,
            Entry::Symlink { path, .. } => path,
            Entry::Directory { path, .. } => path,
        }
    }

    fn depth(&self) -> usize {
        match self {
            Entry::File { depth, .. } => *depth,
            Entry::Symlink { depth, .. } => *depth,
            Entry::Directory { depth, .. } => *depth,
        }
    }
}

#[allow(clippy::large_enum_variant)]
enum State {
    Idle(Option<(VecDeque<io::Result<Entry>>, IntoIter, bool)>),
    Pending(JoinHandle<(VecDeque<io::Result<Entry>>, IntoIter, bool)>),
}

const CHUNK_SIZE: usize = 25;

impl State {
    fn poll_next(&mut self, cx: &mut Context<'_>) -> Poll<Option<io::Result<Entry>>> {
        loop {
            match self {
                State::Idle(data) => {
                    let (buf, _, remain) = data.as_mut().unwrap();
                    if let Some(entry) = buf.pop_front() {
                        return Poll::Ready(Some(entry));
                    } else if !*remain {
                        return Poll::Ready(None);
                    }
                    let (mut buf, mut walker, _) = data.take().unwrap();
                    *self = State::Pending(spawn_blocking(|| {
                        let remain = State::next_chunk(&mut buf, &mut walker);
                        (buf, walker, remain)
                    }));
                }
                State::Pending(handler) => {
                    *self = State::Idle(Some(ready!(Pin::new(handler).poll(cx))?));
                }
            }
        }
    }
    fn next_chunk(buf: &mut VecDeque<io::Result<Entry>>, iter: &mut IntoIter) -> bool {
        for _ in 0..CHUNK_SIZE {
            match iter.next() {
                Some(res) => {
                    let res = res.map_err(io::Error::from).and_then(|entry| {
                        let depth = entry.depth();
                        let m = entry.metadata()?;
                        if m.is_dir() {
                            Ok(Entry::Directory {
                                depth,
                                path: entry.into_path(),
                            })
                        } else if m.is_file() {
                            let executable;
                            #[cfg(unix)]
                            {
                                let mode = m.permissions().mode();
                                executable = mode & 0o100 == 0o100;
                            }
                            #[cfg(not(unix))]
                            {
                                executable = false;
                            }
                            let size = m.len();
                            let path = entry.into_path();
                            // Load the content here, in the same blocking
                            // task that is already iterating the directory,
                            // so the async side receives ready-to-stream
                            // bytes without a second pool round-trip.
                            let data = load_file_bytes(&path, size)?;
                            Ok(Entry::File {
                                depth,
                                path,
                                size,
                                executable,
                                data,
                            })
                        } else if m.is_symlink() {
                            let target = read_link(entry.path())?;
                            Ok(Entry::Symlink {
                                depth,
                                path: entry.into_path(),
                                target,
                            })
                        } else {
                            Err(io::Error::other(format!(
                                "unsupported file type {:?}",
                                m.file_type()
                            )))
                        }
                    });
                    buf.push_back(res);
                }
                None => return false,
            }
        }
        true
    }
}

pub struct NarDumper {
    state: State,
    next: Option<Entry>,
    level: u32,
    use_case_hack: bool,
}

impl NarDumper {
    pub fn new<P>(root: P) -> Self
    where
        P: Into<PathBuf>,
    {
        DumpOptions::new().dump(root)
    }
}

impl Stream for NarDumper {
    type Item = io::Result<NarEvent<DumpedFile>>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        loop {
            // Close directories until the pending entry is at the current
            // nesting level. walkdir's `depth()` gives this directly, so no
            // path comparison is needed.
            if let Some(entry) = self.next.as_ref()
                && entry.depth() < self.level as usize
            {
                self.level -= 1;
                return Poll::Ready(Some(Ok(NarEvent::EndDirectory)));
            }
            if let Some(entry) = self.next.take() {
                let name = if self.level > 0 {
                    let mut name = Bytes::copy_from_slice(fast_file_name(entry.path()));
                    if self.use_case_hack {
                        remove_case_hack(&mut name);
                    }
                    name
                } else {
                    Bytes::new()
                };
                let event = match entry {
                    Entry::Directory { path: _, .. } => {
                        self.level += 1;
                        NarEvent::StartDirectory { name }
                    }
                    Entry::File {
                        size,
                        executable,
                        data,
                        ..
                    } => {
                        let reader = DumpedFile::new(data);
                        NarEvent::File {
                            name,
                            executable,
                            size,
                            reader,
                        }
                    }
                    Entry::Symlink { target, .. } => {
                        let target = Vec::from_os_string(target.into_os_string())
                            .map_err(|target_s| {
                                io::Error::other(format!("target {target_s:?} not valid UTF-8"))
                            })?
                            .into();

                        NarEvent::Symlink { name, target }
                    }
                };
                return Poll::Ready(Some(Ok(event)));
            }
            match ready!(self.state.poll_next(cx)) {
                Some(Ok(entry)) => {
                    self.next = Some(entry);
                }
                Some(Err(err)) => return Poll::Ready(Some(Err(err))),
                None => {
                    if self.level > 0 {
                        self.level -= 1;
                        return Poll::Ready(Some(Ok(NarEvent::EndDirectory)));
                    }
                    return Poll::Ready(None);
                }
            }
        }
    }
}

#[cfg(test)]
mod unittests {
    use std::fs::create_dir_all;

    use futures_util::TryStreamExt as _;
    use tempfile::Builder;

    use super::*;
    use crate::archive::test_data;

    #[tokio::test]
    async fn test_dump_dir() {
        let dir = Builder::new().prefix("test_dump_dir").tempdir().unwrap();
        let path = dir.path().join("nar");
        test_data::create_dir_example(&path, true).unwrap();

        let s = DumpOptions::new()
            .use_case_hack(true)
            .dump(path)
            .and_then(|entry| entry.read_file())
            .try_collect::<test_data::TestNarEvents>()
            .await
            .unwrap();
        assert_eq!(s, test_data::dir_example());
    }

    #[tokio::test]
    async fn test_dump_text_file() {
        let dir = Builder::new()
            .prefix("test_dump_text_file")
            .tempdir()
            .unwrap();
        let path = dir.path().join("nar");
        test_data::create_dir_example(&path, true).unwrap();

        let s = dump(path.join("testing.txt"))
            .and_then(|entry| entry.read_file())
            .try_collect::<test_data::TestNarEvents>()
            .await
            .unwrap();
        assert_eq!(s, test_data::text_file());
    }

    #[tokio::test]
    async fn test_dump_exec_file() {
        let dir = Builder::new()
            .prefix("test_dump_exec_file")
            .tempdir()
            .unwrap();
        let path = dir.path().join("nar");
        test_data::create_dir_example(&path, true).unwrap();

        let s = dump(path.join("dir/more/Deep"))
            .and_then(|entry| entry.read_file())
            .try_collect::<test_data::TestNarEvents>()
            .await
            .unwrap();
        assert_eq!(s, test_data::exec_file());
    }

    #[tokio::test]
    async fn test_dump_empty_file() {
        let dir = Builder::new()
            .prefix("test_dump_empty_file")
            .tempdir()
            .unwrap();
        let path = dir.path().join("empty.keep");
        std::fs::write(&path, b"").unwrap();

        let s = dump(path)
            .and_then(|entry| entry.read_file())
            .try_collect::<test_data::TestNarEvents>()
            .await
            .unwrap();
        assert_eq!(s, test_data::empty_file());
    }

    #[tokio::test]
    async fn test_dump_symlink() {
        let dir = Builder::new()
            .prefix("test_dump_symlink")
            .tempdir()
            .unwrap();
        let deep = dir.path().join("deep");
        create_dir_all(&deep).unwrap();
        let path = deep.join("loop");
        std::os::unix::fs::symlink("../deep", &path).unwrap();

        let s = dump(path)
            .and_then(|entry| entry.read_file())
            .try_collect::<test_data::TestNarEvents>()
            .await
            .unwrap();
        assert_eq!(s, test_data::symlink());
    }
}
