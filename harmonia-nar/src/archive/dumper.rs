use std::cmp::Ordering;
use std::ffi::OsStr;
use std::fs::read_link;
use std::future::Future as _;
use std::os::unix::ffi::OsStrExt;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, ready};
use std::{collections::VecDeque, io};

use bstr::{ByteSlice as _, ByteVec as _};
use bytes::Bytes;
use futures_core::Stream;
use pin_project_lite::pin_project;
use tokio::io::AsyncRead;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::task::{JoinHandle, spawn_blocking};
use tokio_util::sync::PollSemaphore;
use tracing::debug;
use walkdir::{DirEntry, IntoIter};

use super::{CASE_HACK_SUFFIX, NarEvent};

pub struct DumpOptions {
    use_case_hack: bool,
    max_open_files: usize,
}

impl DumpOptions {
    pub fn new() -> Self {
        #[cfg(target_os = "macos")]
        let use_case_hack = true;
        #[cfg(not(target_os = "macos"))]
        let use_case_hack = false;
        Self {
            use_case_hack,
            max_open_files: OPEN_FILES,
        }
    }

    pub fn use_case_hack(mut self, use_case_hack: bool) -> Self {
        self.use_case_hack = use_case_hack;
        self
    }

    pub fn max_open_files(mut self, max_open_files: usize) -> Self {
        self.max_open_files = max_open_files;
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
            semaphore: Arc::new(Semaphore::new(self.max_open_files)),
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

/// Files smaller than this are read into memory in one `spawn_blocking` call,
/// avoiding per-read context switches. Larger files use mmap for zero-copy
/// streaming without unbounded memory allocation.
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

pin_project! {
    #[project = DumpedFileStatesProj]
    enum DumpedFileStates {
        WaitPermit {
            #[pin]
            semaphore: PollSemaphore,
            file: Option<(PathBuf, u64)>,
        },
        OpenFile {
            #[pin]
            handle: JoinHandle<io::Result<(Bytes, OwnedSemaphorePermit)>>,
        },
        Reading {
            data: Bytes,
            offset: usize,
            _permit: OwnedSemaphorePermit,
        },
        Eof,
    }
}

pin_project! {
    pub struct DumpedFile {
        #[pin]
        states: DumpedFileStates,
    }
}

impl DumpedFile {
    pub fn new<P>(path: P, size: u64, semaphore: Arc<Semaphore>) -> Self
    where
        P: Into<PathBuf>,
    {
        Self {
            states: DumpedFileStates::WaitPermit {
                semaphore: PollSemaphore::new(semaphore),
                file: Some((path.into(), size)),
            },
        }
    }
}

impl DumpedFile {
    /// Drive the open/mmap state machine until file data is available.
    /// Shared by both [`AsyncRead`] and [`DumpedFile::poll_bytes`].
    fn poll_load(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let mut this = self.project();
        loop {
            match this.states.as_mut().project() {
                DumpedFileStatesProj::WaitPermit {
                    mut semaphore,
                    file,
                } => match ready!(semaphore.poll_acquire(cx)) {
                    Some(permit) => {
                        let (path, size) = file.take().unwrap();
                        let handle =
                            spawn_blocking(move || Ok((load_file_bytes(&path, size)?, permit)));
                        this.states.set(DumpedFileStates::OpenFile { handle });
                    }
                    None => {
                        this.states.set(DumpedFileStates::Eof);
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::BrokenPipe,
                            "semaphore closed",
                        )));
                    }
                },
                DumpedFileStatesProj::OpenFile { handle } => match ready!(handle.poll(cx)) {
                    Ok(Ok((data, permit))) => {
                        this.states.set(DumpedFileStates::Reading {
                            data,
                            offset: 0,
                            _permit: permit,
                        });
                    }
                    Ok(Err(err)) => {
                        this.states.set(DumpedFileStates::Eof);
                        return Poll::Ready(Err(err));
                    }
                    Err(_) => {
                        this.states.set(DumpedFileStates::Eof);
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::BrokenPipe,
                            "spawned task failed",
                        )));
                    }
                },
                DumpedFileStatesProj::Reading { .. } | DumpedFileStatesProj::Eof => {
                    return Poll::Ready(Ok(()));
                }
            }
        }
    }

    /// Resolve the entire file content as a single zero-copy [`Bytes`].
    ///
    /// For small files this is the heap buffer moved into `Bytes`; for large
    /// files it's a refcounted view over the mmap. The semaphore permit is
    /// released immediately since neither holds an open file descriptor.
    pub fn poll_bytes(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<Bytes>> {
        ready!(self.as_mut().poll_load(cx))?;
        let mut this = self.project();
        match this.states.as_mut().project() {
            DumpedFileStatesProj::Reading { data, .. } => {
                let out = std::mem::take(data);
                this.states.set(DumpedFileStates::Eof);
                Poll::Ready(Ok(out))
            }
            DumpedFileStatesProj::Eof => Poll::Ready(Ok(Bytes::new())),
            _ => unreachable!("poll_load returned Ready without reaching terminal state"),
        }
    }
}

impl AsyncRead for DumpedFile {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        ready!(self.as_mut().poll_load(cx))?;
        let mut this = self.project();
        if let DumpedFileStatesProj::Reading { data, offset, .. } = this.states.as_mut().project() {
            let remaining = &data[*offset..];
            if remaining.is_empty() {
                this.states.set(DumpedFileStates::Eof);
            } else {
                let to_copy = remaining.len().min(buf.remaining());
                buf.put_slice(&remaining[..to_copy]);
                *offset += to_copy;
            }
        }
        Poll::Ready(Ok(()))
    }
}

#[derive(Debug)]
enum Entry {
    File {
        depth: usize,
        path: PathBuf,
        size: u64,
        executable: bool,
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
                            Ok(Entry::File {
                                depth,
                                path: entry.into_path(),
                                size: m.len(),
                                executable,
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
    semaphore: Arc<Semaphore>,
    use_case_hack: bool,
}

const OPEN_FILES: usize = 100;

impl NarDumper {
    pub fn new<P>(root: P) -> Self
    where
        P: Into<PathBuf>,
    {
        Self::with_max_open_files(root, OPEN_FILES)
    }

    pub fn with_max_open_files<P>(root: P, max_open_files: usize) -> Self
    where
        P: Into<PathBuf>,
    {
        DumpOptions::new().max_open_files(max_open_files).dump(root)
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
                        path,
                        size,
                        executable,
                        ..
                    } => {
                        let reader = DumpedFile::new(path, size, self.semaphore.clone());
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
