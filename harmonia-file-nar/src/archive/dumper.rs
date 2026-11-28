use std::ffi::CStr;
use std::future::Future as _;
use std::mem::MaybeUninit;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::task::{Context, Poll, ready};
use std::{collections::VecDeque, io};

use bstr::ByteSlice as _;
use bytes::{Bytes, BytesMut};
use futures_core::Stream;
use rustix::fs::{
    AtFlags, CWD, FileType, Mode, OFlags, fstat, openat, readlinkat, readlinkat_raw, statat,
};
use rustix::io::Errno;
use tokio::io::AsyncRead;
use tokio::task::{JoinHandle, spawn_blocking};
use tracing::debug;

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
        NarDumper {
            state: State::Idle(Some(Box::new(BatchState {
                buf: VecDeque::with_capacity(CHUNK_SIZE),
                walker: DirWalker::new(path.into(), self.use_case_hack),
                remain: true,
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

/// Files up to this size are read eagerly; larger files are streamed in
/// bounded chunks so memory stays bounded.
///
/// Deliberately `read()`-based, not mmap: the kernel patches uprobe
/// breakpoint bytes into every mapping of a probed inode (even `PROT_READ`),
/// so an mmap-based dumper serves NARs that do not match the signed
/// `NarHash` (issue #1140). Chunked reads also benchmark at least as fast
/// as mmap for this one-pass workload.
/// Chunk / eager-read threshold (256 KiB); also imported by the restorer.
pub(crate) const FILE_CHUNK_SIZE: usize = 256 * 1024;
pub(crate) const SMALL_FILE_THRESHOLD: u64 = FILE_CHUNK_SIZE as u64;

/// Streaming chunk size kept below 128 KiB so repeated single-chunk
/// allocations stay in the heap cache instead of triggering per-chunk
/// `mmap`/`munmap` page faults.
pub(crate) const STREAM_FILE_CHUNK_SIZE: usize = 64 * 1024;

/// Small eager files up to this size are packed into a bounded batch slab in
/// `NarDumper` rather than allocating a standalone buffer per file.
const SLAB_INLINE_LIMIT: usize = 16 * 1024;

/// Maximum size of a single shared batch slab in `NarDumper`, bounding memory
/// retention if a consumer holds onto one entry's `Bytes`.
const MAX_SLAB_BYTES: usize = 64 * 1024;

const DIR_OPEN_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);

/// `O_NONBLOCK` prevents blocking a worker thread if an entry is swapped for a
/// FIFO between `readdir` and `openat`; on POSIX it has no effect on regular
/// files once `fstat` verifies `FileType::RegularFile`.
const FILE_OPEN_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::NOFOLLOW)
    .union(OFlags::NONBLOCK)
    .union(OFlags::CLOEXEC);

#[cfg(any(target_os = "linux", target_os = "android"))]
const RAW_DIR_BUF_LEN: usize = 32 * 1024;

/// Read from `fd` into `buf`'s uninitialized spare capacity until
/// `buf.len() >= target`, returning an error on unexpected EOF.
fn read_uninit_until(fd: BorrowedFd<'_>, buf: &mut Vec<u8>, target: usize) -> io::Result<()> {
    while buf.len() < target {
        match rustix::io::read(fd, rustix::buffer::spare_capacity(buf)) {
            Ok(0) => {
                return Err(io::Error::other(format!(
                    "file truncated during dump: {} bytes missing",
                    target - buf.len()
                )));
            }
            Ok(_) => {}
            Err(Errno::INTR) => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

/// Read an entire file of `size` bytes into `buf`'s uninitialized spare
/// capacity without zero-initializing first.
///
/// The caller must close `fd` after this call: if the file grew concurrently
/// past `size`, `spare_capacity` may read past `size` before truncating `buf`
/// back to `start + size`.
pub(crate) fn read_whole_file_uninit(
    fd: BorrowedFd<'_>,
    buf: &mut Vec<u8>,
    size: usize,
) -> io::Result<()> {
    if size == 0 {
        return Ok(());
    }
    let target = buf.len() + size;
    buf.reserve(size);
    read_uninit_until(fd, buf, target)?;
    buf.truncate(target);
    Ok(())
}

/// Read exactly `want` bytes from `fd` into a fresh `Vec<u8>` without
/// zero-initializing it first. Safe to call repeatedly on the same `fd` because
/// `Vec::with_capacity(want)` exposes at most `want` bytes of spare capacity.
pub(crate) fn read_chunk_uninit(fd: BorrowedFd<'_>, want: usize) -> io::Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(want);
    if buf.capacity() == want {
        read_uninit_until(fd, &mut buf, want)?;
    } else {
        buf.resize(want, 0);
        let mut off = 0;
        while off < want {
            match rustix::io::read(fd, &mut buf[off..want]) {
                Ok(0) => {
                    return Err(io::Error::other(format!(
                        "file truncated during dump: {} bytes missing",
                        want - off
                    )));
                }
                Ok(n) => off += n,
                Err(Errno::INTR) => continue,
                Err(e) => return Err(e.into()),
            }
        }
    }
    Ok(buf)
}

#[derive(Clone, Copy)]
struct DirItem {
    name_start: u32,
    disk_len: u32,
    nar_len: u32,
    file_type: FileType,
}

#[inline]
fn nar_slice(names: &[u8], item: DirItem) -> &[u8] {
    let start = item.name_start as usize;
    &names[start..start + item.nar_len as usize]
}

#[inline]
fn disk_slice(names: &[u8], item: DirItem) -> &[u8] {
    let start = item.name_start as usize;
    &names[start..start + item.disk_len as usize]
}

struct DirFrame {
    dir_fd: OwnedFd,
    /// NUL-terminated on-disk entry names packed back-to-back so each item's
    /// slice can be passed directly to `openat`/`readlinkat`/`statat` as a
    /// `&CStr` with zero allocations.
    names: Vec<u8>,
    items: Vec<DirItem>,
    next_idx: usize,
}

impl DirFrame {
    #[inline]
    fn nar_name(&self, item: DirItem) -> &[u8] {
        nar_slice(&self.names, item)
    }

    #[inline]
    fn disk_cstr(&self, item: DirItem) -> &CStr {
        let start = item.name_start as usize;
        CStr::from_bytes_with_nul(&self.names[start..start + item.disk_len as usize + 1])
            .expect("DirFrame packs NUL-terminated names")
    }
}

pub(crate) enum WalkEvent<'a> {
    Directory {
        depth: usize,
        name: &'a [u8],
    },
    File {
        depth: usize,
        name: &'a [u8],
        fd: OwnedFd,
        size: u64,
        executable: bool,
    },
    Symlink {
        depth: usize,
        name: &'a [u8],
        target: &'a [u8],
    },
}

/// Read a symlink target into `out` without allocating on the common path
/// (targets shorter than 4 KiB).
fn read_symlink_into(
    dir_fd: BorrowedFd<'_>,
    name: impl rustix::path::Arg + Copy,
    out: &mut Vec<u8>,
) -> io::Result<()> {
    const STACK_LEN: usize = 4096;
    let mut stack = [MaybeUninit::<u8>::uninit(); STACK_LEN];
    let (init, _) = readlinkat_raw(dir_fd, name, &mut stack)?;
    if init.len() < STACK_LEN {
        out.clear();
        out.extend_from_slice(init);
        return Ok(());
    }
    // Target filled the stack buffer; retry with growing heap buffer.
    let cstr = readlinkat(dir_fd, name, Vec::new())?;
    *out = cstr.into_bytes();
    Ok(())
}

/// Dirfd-relative depth-first filesystem walker in NAR canonical order.
///
/// Opens each directory once and uses `openat`/`readlinkat`/`statat` relative
/// to the parent directory descriptor, avoiding per-entry `PathBuf`/`CString`
/// allocations and repeated kernel path walks.
pub(crate) struct DirWalker {
    root: Option<PathBuf>,
    use_case_hack: bool,
    frames: Vec<DirFrame>,
    pending_dir: Option<OwnedFd>,
    #[cfg(any(target_os = "linux", target_os = "android"))]
    raw_dir_buf: Option<Box<[MaybeUninit<u8>; RAW_DIR_BUF_LEN]>>,
    symlink_buf: Vec<u8>,
}

impl DirWalker {
    pub(crate) fn new(root: PathBuf, use_case_hack: bool) -> Self {
        Self {
            root: Some(root),
            use_case_hack,
            frames: Vec::new(),
            pending_dir: None,
            #[cfg(any(target_os = "linux", target_os = "android"))]
            raw_dir_buf: None,
            symlink_buf: Vec::new(),
        }
    }

    /// Drop all open directory descriptors; later calls to
    /// [`Self::next_event`] return `Ok(None)`. Callers use this once they
    /// treat the walk as terminal so descriptors are not held until drop.
    pub(crate) fn close(&mut self) {
        self.root = None;
        self.pending_dir = None;
        self.frames.clear();
    }

    #[cfg(test)]
    pub(crate) fn open_dir_count(&self) -> usize {
        self.frames.len() + usize::from(self.pending_dir.is_some())
    }

    fn push_dir(&mut self, dir_fd: OwnedFd) -> io::Result<()> {
        let mut names = Vec::new();
        let mut items = Vec::new();
        let use_case_hack = self.use_case_hack;

        let mut push_entry = |name_bytes: &[u8], file_type: FileType| -> io::Result<()> {
            if name_bytes == b"." || name_bytes == b".." {
                return Ok(());
            }
            let nar_len = if use_case_hack {
                if let Some(pos) = name_bytes.rfind(CASE_HACK_SUFFIX) {
                    debug!(
                        "removing case hack suffix from '{:?}'",
                        name_bytes.as_bstr()
                    );
                    pos
                } else {
                    name_bytes.len()
                }
            } else {
                name_bytes.len()
            };
            let name_start = u32::try_from(names.len())
                .map_err(|_| io::Error::other("directory entry names exceed 4 GiB"))?;
            let disk_len = u32::try_from(name_bytes.len())
                .map_err(|_| io::Error::other("directory entry name too long"))?;
            let nar_len = u32::try_from(nar_len)
                .map_err(|_| io::Error::other("directory entry name too long"))?;
            names.extend_from_slice(name_bytes);
            names.push(0);
            items.push(DirItem {
                name_start,
                disk_len,
                nar_len,
                file_type,
            });
            Ok(())
        };

        #[cfg(any(target_os = "linux", target_os = "android"))]
        {
            let raw_buf = self
                .raw_dir_buf
                .get_or_insert_with(|| Box::new([MaybeUninit::uninit(); RAW_DIR_BUF_LEN]));
            let mut raw = rustix::fs::RawDir::new(&dir_fd, &mut **raw_buf);
            while let Some(entry) = raw.next() {
                let entry = entry?;
                push_entry(entry.file_name().to_bytes(), entry.file_type())?;
            }
        }

        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        {
            for entry in rustix::fs::Dir::read_from(&dir_fd)? {
                let entry = entry?;
                push_entry(entry.file_name().to_bytes(), entry.file_type())?;
            }
        }

        items.sort_unstable_by(|&a, &b| {
            nar_slice(&names, a)
                .cmp(nar_slice(&names, b))
                .then_with(|| disk_slice(&names, a).cmp(disk_slice(&names, b)))
        });

        self.frames.push(DirFrame {
            dir_fd,
            names,
            items,
            next_idx: 0,
        });
        Ok(())
    }

    fn open_regular_and_stat(
        dir_fd: BorrowedFd<'_>,
        name: impl rustix::path::Arg,
    ) -> io::Result<(OwnedFd, u64, bool)> {
        let fd = openat(dir_fd, name, FILE_OPEN_FLAGS, Mode::empty())?;
        let st = fstat(&fd)?;
        let ft = FileType::from_raw_mode(st.st_mode);
        if ft != FileType::RegularFile {
            return Err(io::Error::other(format!(
                "file type changed during dump: {ft:?}"
            )));
        }
        let executable = Mode::from_raw_mode(st.st_mode).contains(Mode::XUSR);
        let size = u64::try_from(st.st_size)
            .map_err(|_| io::Error::other("negative file size reported by fstat"))?;
        Ok((fd, size, executable))
    }

    fn next_root(&mut self, root: &Path) -> io::Result<WalkEvent<'_>> {
        let st = statat(CWD, root, AtFlags::SYMLINK_NOFOLLOW)?;
        match FileType::from_raw_mode(st.st_mode) {
            FileType::Directory => {
                let fd = openat(CWD, root, DIR_OPEN_FLAGS, Mode::empty())?;
                self.pending_dir = Some(fd);
                Ok(WalkEvent::Directory {
                    depth: 0,
                    name: b"",
                })
            }
            FileType::RegularFile => {
                let (fd, size, executable) = Self::open_regular_and_stat(CWD, root)?;
                Ok(WalkEvent::File {
                    depth: 0,
                    name: b"",
                    fd,
                    size,
                    executable,
                })
            }
            FileType::Symlink => {
                read_symlink_into(CWD, root, &mut self.symlink_buf)?;
                Ok(WalkEvent::Symlink {
                    depth: 0,
                    name: b"",
                    target: &self.symlink_buf,
                })
            }
            other => Err(io::Error::other(format!("unsupported file type {other:?}"))),
        }
    }

    pub(crate) fn next_event(&mut self) -> io::Result<Option<WalkEvent<'_>>> {
        if let Some(root) = self.root.take() {
            return self.next_root(&root).map(Some);
        }

        if let Some(dir_fd) = self.pending_dir.take() {
            self.push_dir(dir_fd)?;
        }

        while let Some(top) = self.frames.last() {
            if top.next_idx < top.items.len() {
                break;
            }
            self.frames.pop();
        }

        let depth = self.frames.len();
        let Some(frame) = self.frames.last_mut() else {
            return Ok(None);
        };
        let item = frame.items[frame.next_idx];
        frame.next_idx += 1;

        let c_name = frame.disk_cstr(item);
        let file_type = match item.file_type {
            FileType::Unknown => {
                let st = statat(&frame.dir_fd, c_name, AtFlags::SYMLINK_NOFOLLOW)?;
                FileType::from_raw_mode(st.st_mode)
            }
            ft => ft,
        };

        match file_type {
            FileType::Directory => {
                let child_fd = openat(&frame.dir_fd, c_name, DIR_OPEN_FLAGS, Mode::empty())?;
                let name = frame.nar_name(item);
                // Defer reading the child directory until the next call so
                // `self.frames` is not mutated while `name` borrows from it.
                self.pending_dir = Some(child_fd);
                Ok(Some(WalkEvent::Directory { depth, name }))
            }
            FileType::RegularFile => {
                let (fd, size, executable) =
                    Self::open_regular_and_stat(frame.dir_fd.as_fd(), c_name)?;
                let name = frame.nar_name(item);
                Ok(Some(WalkEvent::File {
                    depth,
                    name,
                    fd,
                    size,
                    executable,
                }))
            }
            FileType::Symlink => {
                read_symlink_into(frame.dir_fd.as_fd(), c_name, &mut self.symlink_buf)?;
                let name = frame.nar_name(item);
                Ok(Some(WalkEvent::Symlink {
                    depth,
                    name,
                    target: &self.symlink_buf,
                }))
            }
            other => Err(io::Error::other(format!("unsupported file type {other:?}"))),
        }
    }
}

enum FileData {
    Eager(Bytes),
    /// Opened by the blocking walker task, read chunk-by-chunk during emission.
    Lazy {
        fd: OwnedFd,
        remaining: u64,
    },
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
    /// Produce the next content chunk, or `None` when the file is fully
    /// emitted.
    pub(crate) fn next_chunk(&mut self) -> Option<io::Result<Bytes>> {
        match &mut self.data {
            FileData::Eager(bytes) => {
                if bytes.is_empty() {
                    return None;
                }
                let n = bytes.len().min(STREAM_FILE_CHUNK_SIZE);
                Some(Ok(bytes.split_to(n)))
            }
            FileData::Lazy { fd, remaining } => {
                if *remaining == 0 {
                    return None;
                }
                let want = (*remaining).min(STREAM_FILE_CHUNK_SIZE as u64) as usize;
                Some(read_chunk_uninit(fd.as_fd(), want).map(|buf| {
                    *remaining -= want as u64;
                    Bytes::from(buf)
                }))
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

/// Bounded `BytesMut` slab for packing small entry names, symlink targets, and
/// small file bodies without per-entry heap allocations or unbounded retention.
struct BatchSlab {
    buf: BytesMut,
}

impl BatchSlab {
    fn new() -> Self {
        Self {
            buf: BytesMut::with_capacity(MAX_SLAB_BYTES),
        }
    }

    fn copy(&mut self, src: &[u8]) -> Bytes {
        if src.is_empty() {
            return Bytes::new();
        }
        if self.buf.capacity() < src.len() {
            self.buf = BytesMut::with_capacity(MAX_SLAB_BYTES.max(src.len()));
        }
        self.buf.extend_from_slice(src);
        self.buf.split().freeze()
    }
}

struct BatchState {
    buf: VecDeque<io::Result<Entry>>,
    walker: DirWalker,
    remain: bool,
}

#[allow(clippy::large_enum_variant)]
enum State {
    Idle(Option<Box<BatchState>>),
    Pending(JoinHandle<Box<BatchState>>),
}

const CHUNK_SIZE: usize = 512;
const MAX_BATCH_EAGER_BYTES: usize = 4 * 1024 * 1024;
/// Each lazy entry keeps its file descriptor open until it is emitted, so cap
/// them per batch to bound descriptors per dump.
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
                        st.remain = State::next_chunk(&mut st.buf, &mut st.walker);
                        st
                    }));
                }
                State::Pending(handler) => {
                    *self = State::Idle(Some(ready!(Pin::new(handler).poll(cx))?));
                }
            }
        }
    }

    fn next_chunk(buf: &mut VecDeque<io::Result<Entry>>, walker: &mut DirWalker) -> bool {
        let mut eager_bytes = 0usize;
        let mut lazy_files = 0usize;
        let mut slab = BatchSlab::new();
        let mut scratch = Vec::new();

        for _ in 0..CHUNK_SIZE {
            match walker.next_event() {
                Ok(Some(ev)) => {
                    let res = match ev {
                        WalkEvent::Directory { depth, name } => Ok(Entry::Directory {
                            depth,
                            name: slab.copy(name),
                        }),
                        WalkEvent::Symlink {
                            depth,
                            name,
                            target,
                        } => Ok(Entry::Symlink {
                            depth,
                            name: slab.copy(name),
                            target: slab.copy(target),
                        }),
                        WalkEvent::File {
                            depth,
                            name,
                            fd,
                            size,
                            executable,
                        } => {
                            let name = slab.copy(name);
                            let data_res = if size == 0 {
                                Ok(FileData::Eager(Bytes::new()))
                            } else if size <= SLAB_INLINE_LIMIT as u64 {
                                let n = size as usize;
                                scratch.clear();
                                read_whole_file_uninit(fd.as_fd(), &mut scratch, n).map(|()| {
                                    eager_bytes = eager_bytes.saturating_add(n);
                                    FileData::Eager(slab.copy(&scratch))
                                })
                            } else if size <= SMALL_FILE_THRESHOLD {
                                let n = size as usize;
                                read_chunk_uninit(fd.as_fd(), n).map(|data| {
                                    eager_bytes = eager_bytes.saturating_add(n);
                                    FileData::Eager(Bytes::from(data))
                                })
                            } else {
                                lazy_files += 1;
                                Ok(FileData::Lazy {
                                    fd,
                                    remaining: size,
                                })
                            };
                            data_res.map(|data| Entry::File {
                                depth,
                                name,
                                size,
                                executable,
                                data,
                            })
                        }
                    };
                    buf.push_back(res);
                    if eager_bytes >= MAX_BATCH_EAGER_BYTES || lazy_files >= MAX_BATCH_LAZY_FILES {
                        return true;
                    }
                }
                Ok(None) => return false,
                Err(e) => {
                    walker.close();
                    buf.push_back(Err(e));
                    return false;
                }
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
pub(crate) mod unittests {
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

    pub(crate) fn mkfifo(path: &Path) {
        // rustix has no mknodat on Apple targets, so use mkfifo(1) there.
        #[cfg(not(target_vendor = "apple"))]
        rustix::fs::mknodat(CWD, path, FileType::Fifo, Mode::RUSR | Mode::WUSR, 0).unwrap();
        #[cfg(target_vendor = "apple")]
        assert!(
            std::process::Command::new("mkfifo")
                .arg(path)
                .status()
                .unwrap()
                .success()
        );
    }

    #[tokio::test]
    async fn test_dump_rejects_fifo() {
        let dir = Builder::new().prefix("test_dump_fifo").tempdir().unwrap();
        let fifo = dir.path().join("pipe");
        mkfifo(&fifo);

        let err = dump(&fifo)
            .try_collect::<Vec<_>>()
            .await
            .err()
            .expect("FIFO root must fail");
        assert!(err.to_string().contains("unsupported file type"), "{err}");

        let err = dump(dir.path())
            .try_collect::<Vec<_>>()
            .await
            .err()
            .expect("FIFO child must fail");
        assert!(err.to_string().contains("unsupported file type"), "{err}");
    }

    /// A terminal walker error must release every open directory descriptor
    /// instead of holding them until the exhausted dumper is dropped.
    #[test]
    fn walker_error_releases_dir_fds() {
        let dir = Builder::new().prefix("test_walk_err").tempdir().unwrap();
        let nested = dir.path().join("a/b/c");
        create_dir_all(&nested).unwrap();
        mkfifo(&nested.join("pipe"));

        let mut walker = DirWalker::new(dir.path().to_path_buf(), false);
        let mut buf = VecDeque::new();
        while State::next_chunk(&mut buf, &mut walker) {}
        let err = buf.pop_back().unwrap().err().expect("FIFO must fail");
        assert!(err.to_string().contains("unsupported file type"), "{err}");
        assert_eq!(walker.open_dir_count(), 0);
        assert!(walker.next_event().unwrap().is_none());
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
        let mut walker = DirWalker::new(dir.path().to_path_buf(), false);
        let mut total = 0;
        loop {
            let mut buf = VecDeque::new();
            let remain = State::next_chunk(&mut buf, &mut walker);
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
