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
use futures::Stream;
use pin_project_lite::pin_project;
use tokio::fs;
use tokio::io::{AsyncBufRead, AsyncRead, BufReader};
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
        let dir = root.clone();
        let mut walker = walkdir::WalkDir::new(&root)
            .follow_links(false)
            .follow_root_links(false);
        walker = if self.use_case_hack {
            walker.sort_by(sort_case_hack)
        } else {
            walker.sort_by_file_name()
        };
        let walker = walker.into_iter();
        NarDumper {
            state: State::Idle(Some((VecDeque::with_capacity(CHUNK_SIZE), walker, true))),
            next: None,
            level: 0,
            dir,
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

pub fn dump<P: Into<PathBuf>>(
    path: P,
) -> impl Stream<Item = io::Result<NarEvent<impl AsyncBufRead>>> {
    DumpOptions::new().dump(path)
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

pin_project! {
    #[project = DumpedFileStatesProj]
    enum DumpedFileStates {
        WaitPermit {
            #[pin]
            semaphore: PollSemaphore,
            file: Option<PathBuf>,
        },
        OpenFile {
            permit: Option<OwnedSemaphorePermit>,
            #[pin]
            handle: JoinHandle<io::Result<std::fs::File>>,
        },
        Reading {
            #[pin]
            file: fs::File,
            permit: OwnedSemaphorePermit,
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
    pub fn new<P>(path: P, semaphore: Arc<Semaphore>) -> Self
    where
        P: Into<PathBuf>,
    {
        Self {
            states: DumpedFileStates::WaitPermit {
                semaphore: PollSemaphore::new(semaphore),
                file: Some(path.into()),
            },
        }
    }
}

impl AsyncRead for DumpedFile {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let mut this = self.project();
        loop {
            match this.states.as_mut().project() {
                DumpedFileStatesProj::WaitPermit {
                    mut semaphore,
                    file,
                } => match ready!(semaphore.poll_acquire(cx)) {
                    Some(permit) => {
                        let path = file.take().unwrap();
                        let handle = spawn_blocking(|| std::fs::File::open(path));
                        this.states.set(DumpedFileStates::OpenFile {
                            permit: Some(permit),
                            handle,
                        });
                    }
                    None => {
                        this.states.set(DumpedFileStates::Eof);
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::BrokenPipe,
                            "semaphore closed",
                        )));
                    }
                },
                DumpedFileStatesProj::OpenFile { permit, handle } => {
                    match ready!(handle.poll(cx)) {
                        Ok(Ok(file)) => {
                            let file = fs::File::from_std(file);
                            let permit = permit.take().unwrap();
                            this.states.set(DumpedFileStates::Reading { file, permit });
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
                    }
                }
                DumpedFileStatesProj::Reading { file, permit: _ } => {
                    let filled = buf.filled().len();
                    ready!(file.poll_read(cx, buf))?;
                    if filled == buf.filled().len() {
                        this.states.set(DumpedFileStates::Eof);
                    }
                    break;
                }
                DumpedFileStatesProj::Eof => break,
            }
        }
        Poll::Ready(Ok(()))
    }
}

#[derive(Debug)]
enum Entry {
    File {
        path: PathBuf,
        size: u64,
        executable: bool,
    },
    Symlink {
        path: PathBuf,
        target: PathBuf,
    },
    Directory {
        path: PathBuf,
    },
}

impl Entry {
    fn path(&self) -> &Path {
        match self {
            Entry::File {
                path,
                size: _,
                executable: _,
            } => path,
            Entry::Symlink { path, target: _ } => path,
            Entry::Directory { path } => path,
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
                        let m = entry.metadata()?;
                        if m.is_dir() {
                            Ok(Entry::Directory {
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
                                path: entry.into_path(),
                                size: m.len(),
                                executable,
                            })
                        } else if m.is_symlink() {
                            let target = read_link(entry.path())?;
                            Ok(Entry::Symlink {
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
    dir: PathBuf,
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
    type Item = io::Result<NarEvent<BufReader<DumpedFile>>>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        loop {
            if let Some(entry) = self.next.as_ref()
                && !entry.path().starts_with(&self.dir)
            {
                self.dir.pop();
                self.level -= 1;
                return Poll::Ready(Some(Ok(NarEvent::EndDirectory)));
            }
            if let Some(entry) = self.next.take() {
                let name = if self.level > 0 {
                    let filename = entry.path().file_name().unwrap();
                    let n = <[u8]>::from_os_str(filename).ok_or_else(|| {
                        io::Error::other(format!("filename {filename:?} not valid UTF-8"))
                    })?;
                    let mut name = Bytes::copy_from_slice(n);
                    if self.use_case_hack {
                        remove_case_hack(&mut name);
                    }
                    name
                } else {
                    Bytes::new()
                };
                let event = match entry {
                    Entry::Directory { path } => {
                        self.dir = path;
                        self.level += 1;
                        NarEvent::StartDirectory { name }
                    }
                    Entry::File {
                        path,
                        size,
                        executable,
                    } => {
                        let reader = BufReader::new(DumpedFile::new(path, self.semaphore.clone()));
                        NarEvent::File {
                            name,
                            executable,
                            size,
                            reader,
                        }
                    }
                    Entry::Symlink { path: _, target } => {
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
                        self.dir.pop();
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

    use futures::TryStreamExt as _;
    use pretty_assertions::assert_eq;
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
