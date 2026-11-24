use std::cmp::Ordering;
use std::ffi::OsStr;
use std::fs::read_link;
use std::future::Future as _;
use std::os::unix::ffi::OsStrExt;
#[cfg(unix)]
use std::os::unix::ffi::OsStringExt;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::task::{Context, Poll, ready};
use std::{collections::VecDeque, io};

use bstr::ByteSlice as _;
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
            state: State::Idle(Some(Box::new(BatchState {
                buf: VecDeque::with_capacity(CHUNK_SIZE),
                walker,
                remain: true,
                use_case_hack: self.use_case_hack,
            }))),
            next: None,
            level: 0,
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

/// Turn a walkdir entry into the NAR entry name.
///
/// The root node carries no name in the NAR format. For deeper entries the
/// final path segment is copied into a fresh small `Bytes`; the entry's
/// `PathBuf` is then dropped here on the blocking thread.
fn entry_name(entry: DirEntry, use_case_hack: bool) -> Bytes {
    if entry.depth() == 0 {
        return Bytes::new();
    }
    let mut name = Bytes::copy_from_slice(fast_file_name(entry.path()));
    if use_case_hack {
        remove_case_hack(&mut name);
    }
    name
}

/// Convert an `OsString` (symlink target) into `Bytes` by moving its buffer.
#[cfg(unix)]
fn os_string_into_bytes(s: std::ffi::OsString) -> Bytes {
    Bytes::from(s.into_vec())
}

/// Files up to this size are read eagerly; larger files are streamed in
/// [`FILE_CHUNK_SIZE`] pieces so memory stays bounded.
///
/// Deliberately `read()`-based, not mmap: the kernel patches uprobe
/// breakpoint bytes into every mapping of a probed inode (even `PROT_READ`),
/// so an mmap-based dumper serves NARs that do not match the signed
/// `NarHash` (issue #1140). Chunked reads also benchmark at least as fast
/// as mmap for this one-pass workload.
const SMALL_FILE_THRESHOLD: u64 = 256 * 1024; // 256 KiB

pub(crate) const FILE_CHUNK_SIZE: usize = 256 * 1024;

enum FileData {
    Eager(Bytes),
    /// Opened by the blocking walker task, read chunk-by-chunk during emission.
    Lazy {
        file: std::fs::File,
        remaining: u64,
    },
}

fn open_regular_file(path: &Path) -> io::Result<std::fs::File> {
    let mut opts = std::fs::OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        opts.custom_flags(nix::fcntl::OFlag::O_NOFOLLOW.bits());
    }
    opts.open(path)
}

fn load_file_data(mut f: std::fs::File, path: &Path, size: u64) -> io::Result<FileData> {
    if size <= SMALL_FILE_THRESHOLD {
        use std::io::Read as _;
        let mut buf = vec![0u8; size as usize];
        // Read the exact `size` bytes advertised in the NAR header in a single
        // syscall (matching the Lazy path) instead of calling `read_to_end`,
        // which issues a second EOF read and grows the buffer.
        if let Err(e) = f.read_exact(&mut buf) {
            return Err(if e.kind() == io::ErrorKind::UnexpectedEof {
                io::Error::other(format!(
                    "file {path:?} truncated during dump: expected {size} bytes"
                ))
            } else {
                e
            });
        }
        Ok(FileData::Eager(Bytes::from(buf)))
    } else {
        Ok(FileData::Lazy {
            file: f,
            remaining: size,
        })
    }
}

/// File contents for a [`NarEvent::File`]: small files are already in memory,
/// large files hold an open descriptor and are read incrementally.
pub struct DumpedFile {
    data: FileData,
    /// Chunk handed out by [`Self::next_chunk`] but not yet fully consumed
    /// through the [`AsyncRead`] interface.
    pending: Bytes,
}

impl DumpedFile {
    /// If the entire file is already in memory and at most `max_len` bytes,
    /// return its bytes so the caller can coalesce it into a framing buffer.
    pub(crate) fn into_small_eager(self, max_len: usize) -> Result<Bytes, Self> {
        match self.data {
            FileData::Eager(bytes) if bytes.len() <= max_len => Ok(bytes),
            data => Err(Self {
                data,
                pending: self.pending,
            }),
        }
    }
    /// Produce the next content chunk, or `None` when the file is fully
    /// emitted. The bounded synchronous `read()` for lazy data is comparable
    /// to the page-fault stalls the previous mmap implementation incurred.
    pub(crate) fn next_chunk(&mut self) -> Option<io::Result<Bytes>> {
        match &mut self.data {
            FileData::Eager(bytes) => {
                if bytes.is_empty() {
                    return None;
                }
                let n = bytes.len().min(FILE_CHUNK_SIZE);
                Some(Ok(bytes.split_to(n)))
            }
            FileData::Lazy { file, remaining } => {
                use std::io::Read as _;
                if *remaining == 0 {
                    return None;
                }
                let want = (*remaining).min(FILE_CHUNK_SIZE as u64) as usize;
                let mut buf = vec![0u8; want];
                // Truncated after lstat; the NAR length prefix is already
                // written, so fail instead of desyncing.
                if let Err(e) = file.read_exact(&mut buf) {
                    return Some(Err(if e.kind() == io::ErrorKind::UnexpectedEof {
                        io::Error::other(format!(
                            "file truncated during dump: {remaining} bytes missing"
                        ))
                    } else {
                        e
                    }));
                }
                *remaining -= want as u64;
                Some(Ok(Bytes::from(buf)))
            }
        }
    }
}

impl AsyncRead for DumpedFile {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.pending.is_empty() {
            match self.next_chunk() {
                None => return Poll::Ready(Ok(())),
                Some(Err(e)) => return Poll::Ready(Err(e)),
                Some(Ok(chunk)) => self.pending = chunk,
            }
        }
        let n = self.pending.len().min(buf.remaining());
        buf.put_slice(&self.pending[..n]);
        bytes::Buf::advance(&mut self.pending, n);
        Poll::Ready(Ok(()))
    }
}

/// A fully prepared directory entry: everything the async side needs to emit
/// the corresponding [`NarEvent`] without touching the filesystem again.
enum Entry {
    File {
        depth: usize,
        name: Bytes,
        size: u64,
        executable: bool,
        data: FileData,
    },
    Symlink {
        depth: usize,
        name: Bytes,
        target: Bytes,
    },
    Directory {
        depth: usize,
        name: Bytes,
    },
}

impl Entry {
    fn depth(&self) -> usize {
        match self {
            Entry::File { depth, .. } => *depth,
            Entry::Symlink { depth, .. } => *depth,
            Entry::Directory { depth, .. } => *depth,
        }
    }
}

struct BatchState {
    buf: VecDeque<io::Result<Entry>>,
    walker: IntoIter,
    remain: bool,
    use_case_hack: bool,
}

#[allow(clippy::large_enum_variant)]
enum State {
    Idle(Option<Box<BatchState>>),
    Pending(JoinHandle<Box<BatchState>>),
}

const CHUNK_SIZE: usize = 256;
const MAX_BATCH_EAGER_BYTES: usize = 4 * 1024 * 1024;
/// Each lazy entry keeps its file descriptor open until it is emitted, so cap
/// them per batch (at the previous `CHUNK_SIZE`) to bound descriptors per dump.
const MAX_BATCH_LAZY_FILES: usize = 25;

impl State {
    fn poll_next(&mut self, cx: &mut Context<'_>) -> Poll<Option<io::Result<Entry>>> {
        loop {
            match self {
                State::Idle(data) => {
                    let st = data.as_mut().unwrap();
                    if let Some(entry) = st.buf.pop_front() {
                        return Poll::Ready(Some(entry));
                    } else if !st.remain {
                        return Poll::Ready(None);
                    }
                    let mut st = data.take().unwrap();
                    *self = State::Pending(spawn_blocking(move || {
                        st.remain =
                            State::next_chunk(&mut st.buf, &mut st.walker, st.use_case_hack);
                        st
                    }));
                }
                State::Pending(handler) => {
                    *self = State::Idle(Some(ready!(Pin::new(handler).poll(cx))?));
                }
            }
        }
    }
    fn next_chunk(
        buf: &mut VecDeque<io::Result<Entry>>,
        iter: &mut IntoIter,
        use_case_hack: bool,
    ) -> bool {
        let mut eager_bytes = 0usize;
        let mut lazy_files = 0usize;
        for _ in 0..CHUNK_SIZE {
            match iter.next() {
                Some(res) => {
                    let res = res.map_err(io::Error::from).and_then(|entry| {
                        let depth = entry.depth();
                        // `file_type()` is cached from `readdir`'s `d_type`
                        // and needs no syscall.
                        let ft = entry.file_type();
                        let entry = if ft.is_dir() {
                            Entry::Directory {
                                depth,
                                name: entry_name(entry, use_case_hack),
                            }
                        } else if ft.is_file() {
                            // Open once and `fstat` the descriptor instead of
                            // doing a separate path-walking `lstat` before `open`.
                            let file = open_regular_file(entry.path())?;
                            let m = file.metadata()?;
                            let executable;
                            #[cfg(unix)]
                            {
                                executable = m.permissions().mode() & 0o100 == 0o100;
                            }
                            #[cfg(not(unix))]
                            {
                                executable = false;
                            }
                            let size = m.len();
                            // Load the content here, in the same blocking
                            // task that is already iterating the directory,
                            // so the async side receives ready-to-stream
                            // bytes without a second pool round-trip.
                            let data = load_file_data(file, entry.path(), size)?;
                            match data {
                                FileData::Eager(ref b) => {
                                    eager_bytes = eager_bytes.saturating_add(b.len());
                                }
                                FileData::Lazy { .. } => lazy_files += 1,
                            }
                            Entry::File {
                                depth,
                                name: entry_name(entry, use_case_hack),
                                size,
                                executable,
                                data,
                            }
                        } else if ft.is_symlink() {
                            let target =
                                os_string_into_bytes(read_link(entry.path())?.into_os_string());
                            Entry::Symlink {
                                depth,
                                name: entry_name(entry, use_case_hack),
                                target,
                            }
                        } else {
                            return Err(io::Error::other(format!("unsupported file type {ft:?}")));
                        };
                        Ok(entry)
                    });
                    buf.push_back(res);
                    if eager_bytes >= MAX_BATCH_EAGER_BYTES || lazy_files >= MAX_BATCH_LAZY_FILES {
                        break;
                    }
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
                let event = match entry {
                    Entry::Directory { name, .. } => {
                        self.level += 1;
                        NarEvent::StartDirectory { name }
                    }
                    Entry::File {
                        name,
                        size,
                        executable,
                        data,
                        ..
                    } => NarEvent::File {
                        name,
                        executable,
                        size,
                        reader: DumpedFile {
                            data,
                            pending: Bytes::new(),
                        },
                    },
                    Entry::Symlink { name, target, .. } => NarEvent::Symlink { name, target },
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

    #[test]
    fn batch_caps_open_lazy_files() {
        const FILES: usize = 2 * MAX_BATCH_LAZY_FILES + 3;
        let dir = Builder::new().prefix("test_lazy_cap").tempdir().unwrap();
        for i in 0..FILES {
            // Sparse files just over the eager threshold become lazy entries.
            let f = std::fs::File::create(dir.path().join(format!("f{i:03}"))).unwrap();
            f.set_len(SMALL_FILE_THRESHOLD + 1).unwrap();
        }
        let mut walker = walkdir::WalkDir::new(dir.path()).into_iter();
        let mut total = 0;
        loop {
            let mut buf = VecDeque::new();
            let remain = State::next_chunk(&mut buf, &mut walker, false);
            let lazy = buf
                .iter()
                .filter(|e| {
                    matches!(
                        e,
                        Ok(Entry::File {
                            data: FileData::Lazy { .. },
                            ..
                        })
                    )
                })
                .count();
            assert!(lazy <= MAX_BATCH_LAZY_FILES, "batch held {lazy} open files");
            total += lazy;
            if !remain {
                break;
            }
        }
        assert_eq!(total, FILES);
    }
}
