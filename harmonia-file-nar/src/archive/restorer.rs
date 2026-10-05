use std::collections::HashMap;
use std::ffi::OsString;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, Write as _};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bstr::ByteSlice as _;
use bytes::Bytes;
use derive_more::Display;
use futures_core::Stream;
use thiserror::Error;
use tokio::io::{AsyncBufRead, AsyncReadExt as _};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};
use tokio::task::{JoinError, spawn_blocking};
use tokio_util::sync::CancellationToken;
use tracing::{debug, trace};

use super::dumper::FILE_CHUNK_SIZE;
use super::{CASE_HACK_SUFFIX, NarEvent};

#[derive(Display, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
pub enum NarWriteOperation {
    #[display("creating directory")]
    CreateDirectory,
    #[display("creating symlink")]
    CreateSymlink,
    #[display("creating file")]
    CreateFile,
    #[display("path contains invalid UTF-8")]
    PathUTF8,
    #[display("invalid NAR entry")]
    InvalidEntry,
}

#[derive(Error, Debug)]
#[error("{operation} {path}: {source}")]
pub struct NarWriteError {
    operation: NarWriteOperation,
    path: PathBuf,
    #[source]
    source: io::Error,
}

impl NarWriteError {
    pub fn new(operation: NarWriteOperation, path: PathBuf, source: io::Error) -> Self {
        Self {
            operation,
            path,
            source,
        }
    }
    pub fn path_utf8_error(path: PathBuf, err: bstr::Utf8Error) -> Self {
        Self::new(
            NarWriteOperation::PathUTF8,
            path,
            io::Error::new(io::ErrorKind::InvalidData, err),
        )
    }
    pub fn invalid_entry_error(path: PathBuf, reason: &'static str) -> Self {
        Self::new(
            NarWriteOperation::InvalidEntry,
            path,
            io::Error::new(io::ErrorKind::InvalidData, reason),
        )
    }
    pub fn create_dir_error(path: PathBuf, err: io::Error) -> Self {
        Self::new(NarWriteOperation::CreateDirectory, path, err)
    }
    pub fn create_symlink_error(path: PathBuf, err: io::Error) -> Self {
        Self::new(NarWriteOperation::CreateSymlink, path, err)
    }
    pub fn create_file_error(path: PathBuf, err: io::Error) -> Self {
        Self::new(NarWriteOperation::CreateFile, path, err)
    }
}

pub struct NarRestorer {
    path: PathBuf,
    use_case_hack: bool,
    entries: Entries,
    dir_stack: Vec<Entries>,
    /// How many directories of the NAR are open.
    depth: usize,
    /// Whether the NAR's root entry has been seen.
    root_seen: bool,
}

impl NarRestorer {
    pub fn new<P: Into<PathBuf>>(path: P) -> Self {
        Self::new_restorer(path, false)
    }

    pub fn with_case_hack<P: Into<PathBuf>>(path: P) -> Self {
        Self::new_restorer(path, true)
    }

    fn new_restorer<P>(path: P, use_case_hack: bool) -> Self
    where
        P: Into<PathBuf>,
    {
        let path = path.into();
        Self {
            path,
            use_case_hack,
            entries: Default::default(),
            dir_stack: Default::default(),
            depth: 0,
            root_seen: false,
        }
    }

    /// Checks that `event` stays inside the destination.
    ///
    /// The events don't have to come from our parser, so the names and the
    /// nesting are checked here: the root has no name, everything below it has
    /// a single path component, and nothing follows the root or closes it twice.
    fn check_event<R>(&mut self, event: &NarEvent<R>) -> Result<(), NarWriteError> {
        let invalid = |name: &[u8], reason| {
            let name = name.to_os_str_lossy();
            NarWriteError::invalid_entry_error(self.path.join(name), reason)
        };
        let name = match event {
            NarEvent::File { name, .. }
            | NarEvent::Symlink { name, .. }
            | NarEvent::StartDirectory { name } => name,
            NarEvent::EndDirectory => {
                if self.depth == 0 {
                    return Err(invalid(b"", "end of a directory that was not started"));
                }
                self.depth -= 1;
                return Ok(());
            }
        };
        if self.depth == 0 {
            if self.root_seen {
                return Err(invalid(name, "entry after the root"));
            }
            if !name.is_empty() {
                return Err(invalid(name, "the root has a name"));
            }
            self.root_seen = true;
        } else if name.is_empty()
            || name.as_ref() == b"."
            || name.as_ref() == b".."
            || name.contains(&b'/')
            || name.contains(&0)
        {
            return Err(invalid(name, "invalid file name"));
        }
        if matches!(event, NarEvent::StartDirectory { .. }) {
            self.depth += 1;
        }
        Ok(())
    }

    /// Process a single NAR event and send its filesystem work to `writer`.
    async fn process_event<R>(
        &mut self,
        event: NarEvent<R>,
        writer: &Writer,
    ) -> Result<(), NarWriteError>
    where
        R: AsyncBufRead + Unpin,
    {
        self.check_event(&event)?;
        match event {
            NarEvent::File {
                name,
                executable,
                size,
                mut reader,
            } => {
                let name = if self.use_case_hack {
                    self.entries.hack_name(name)
                } else {
                    name
                };

                let path = join_name(&self.path, &name)?;
                writer
                    .send(Op::CreateFile(path.clone(), executable))
                    .await?;
                let mut left = size;
                loop {
                    // The parser's reads are small, so this collects bigger
                    // chunks for the writer. Reading to the end of the file
                    // also drains its padding.
                    let mut chunk = Vec::with_capacity(left.min(FILE_CHUNK_SIZE as u64) as usize);
                    (&mut reader)
                        .take(FILE_CHUNK_SIZE as u64)
                        .read_to_end(&mut chunk)
                        .await
                        .map_err(|err| NarWriteError::create_file_error(path.clone(), err))?;
                    left = left.saturating_sub(chunk.len() as u64);
                    let last = chunk.len() < FILE_CHUNK_SIZE;
                    if !chunk.is_empty() {
                        // The permit counts the bytes read, not `size`, so a
                        // reader that returns more than `size` still stays
                        // inside the budget.
                        let permit = writer
                            .data
                            .clone()
                            .acquire_many_owned(chunk.len() as u32)
                            .await
                            .expect("the semaphore is never closed");
                        writer.send(Op::Write(chunk, permit)).await?;
                    }
                    if last {
                        break;
                    }
                }
            }
            NarEvent::Symlink { name, target } => {
                let name = if self.use_case_hack {
                    self.entries.hack_name(name)
                } else {
                    name
                };

                let path = join_name(&self.path, &name)?;
                let target_os = target
                    .to_os_str()
                    .map_err(|err| {
                        let lossy = target.to_os_str_lossy().into_owned();
                        let path = PathBuf::from(lossy);
                        NarWriteError::path_utf8_error(path, err)
                    })?
                    .to_owned();
                writer.send(Op::CreateSymlink(path, target_os)).await?;
            }
            NarEvent::StartDirectory { name } => {
                let name = if self.use_case_hack {
                    let name = self.entries.hack_name(name);

                    #[allow(clippy::mutable_key_type)]
                    let entries = std::mem::take(&mut self.entries);
                    self.dir_stack.push(entries);
                    name
                } else {
                    name
                };

                let path = join_name(&self.path, &name)?;
                self.path = path;
                writer.send(Op::CreateDirectory(self.path.clone())).await?;
            }
            NarEvent::EndDirectory => {
                if self.use_case_hack {
                    self.entries = self.dir_stack.pop().unwrap_or_default();
                }
                self.path.pop();
            }
        }
        Ok(())
    }

    /// Consume a stream of NAR events and restore them to the filesystem.
    ///
    /// One blocking thread does all the filesystem work, which it takes from
    /// a bounded queue. If that thread fails, `restore` returns its error
    /// without waiting for more of the stream. Otherwise `restore` waits for
    /// the thread to finish everything in the queue before it returns, even
    /// with an error. If the caller drops the future instead, the thread
    /// finishes the entry it is working on and skips the rest of the queue.
    pub async fn restore<S, U, R>(mut self, stream: S) -> Result<(), NarWriteError>
    where
        S: Stream<Item = U>,
        U: Into<Result<NarEvent<R>, NarWriteError>>,
        R: AsyncBufRead + Send + Unpin,
    {
        use futures_util::StreamExt as _;
        use futures_util::future::{Either, select};
        let (ops, queued) = mpsc::channel(OPS_IN_FLIGHT);
        let dropped = CancellationToken::new();
        let written = spawn_blocking({
            let dropped = dropped.clone();
            move || write(queued, &dropped)
        });
        let writer = Writer {
            ops,
            data: Arc::new(Semaphore::new(DATA_IN_FLIGHT)),
        };
        let sent = async {
            // This future owns `writer`, so the queue closes when it finishes.
            let writer = writer;
            futures_util::pin_mut!(stream);
            while let Some(item) = stream.next().await {
                let event = item.into()?;
                self.process_event(event, &writer).await?;
            }
            Ok(())
        };
        futures_util::pin_mut!(sent);
        // Locals drop in reverse order, so if the caller drops the future,
        // this cancels the token before `sent` closes the queue. When
        // `restore` returns, the writer thread has already finished, so
        // cancelling does nothing.
        let _cancel_on_drop = dropped.drop_guard();
        // A join error means the writer thread panicked.
        let joined = |written: Result<_, JoinError>| {
            written.unwrap_or_else(|err| std::panic::resume_unwind(err.into_panic()))
        };
        match select(sent, written).await {
            Either::Left((sent, written)) => joined(written.await).and(sent),
            // The writer thread stops early only on an error. Returning it
            // here means `restore` doesn't wait for a stream that stalled.
            Either::Right((written, _)) => joined(written),
        }
    }
}

fn join_name(path: &Path, name: &[u8]) -> Result<PathBuf, NarWriteError> {
    if name.is_empty() {
        Ok(path.to_owned())
    } else {
        let name_os = name.to_os_str().map_err(|err| {
            let lossy = name.to_os_str_lossy();
            let path = path.join(lossy);
            NarWriteError::path_utf8_error(path, err)
        })?;
        Ok(path.join(name_os))
    }
}

/// How many entries the writer thread's queue holds.
const OPS_IN_FLIGHT: usize = 64;

/// How much file data the writer thread's queue holds, so a slow disk can't
/// make a restore use much memory.
const DATA_IN_FLIGHT: usize = 8 * FILE_CHUNK_SIZE;

/// Filesystem work for the writer thread, in NAR order.
enum Op {
    CreateDirectory(PathBuf),
    CreateSymlink(PathBuf, OsString),
    CreateFile(PathBuf, bool),
    /// More data for the last created file. The permit is its share of
    /// [`DATA_IN_FLIGHT`].
    Write(Vec<u8>, OwnedSemaphorePermit),
}

/// The queue to the writer thread, and the budget for the file data in it.
struct Writer {
    ops: mpsc::Sender<Op>,
    data: Arc<Semaphore>,
}

impl Writer {
    /// Queues `op` for the writer thread, waiting while the queue is full.
    ///
    /// This fails only if the writer thread has stopped on an error. `restore`
    /// then returns the thread's error instead of this one.
    async fn send(&self, op: Op) -> Result<(), NarWriteError> {
        self.ops.send(op).await.map_err(|_| {
            NarWriteError::create_file_error(PathBuf::new(), io::ErrorKind::BrokenPipe.into())
        })
    }
}

/// Does the filesystem work in `ops`, in order, with `std::fs`. If `dropped`
/// is cancelled, it returns before the next entry and skips the rest.
fn write(mut ops: mpsc::Receiver<Op>, dropped: &CancellationToken) -> Result<(), NarWriteError> {
    let mut file: Option<(PathBuf, File)> = None;
    while let Some(op) = ops.blocking_recv() {
        // The caller dropped `restore`, so nobody waits for this work, and
        // the caller may already be removing the destination.
        if dropped.is_cancelled() {
            break;
        }
        match op {
            Op::Write(data, _permit) => {
                let (path, file) = file.as_mut().expect("file data follows its file");
                file.write_all(&data)
                    .map_err(|err| NarWriteError::create_file_error(path.clone(), err))?;
            }
            Op::CreateFile(path, executable) => {
                trace!("Writing to file {:?}", path);
                let mut options = OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)]
                options.mode(if executable { 0o777 } else { 0o666 });
                let created = options
                    .open(&path)
                    .map_err(|err| NarWriteError::create_file_error(path.clone(), err))?;
                file = Some((path, created));
            }
            Op::CreateDirectory(path) => {
                file = None;
                std::fs::create_dir(&path)
                    .map_err(|err| NarWriteError::create_dir_error(path, err))?;
            }
            Op::CreateSymlink(path, target) => {
                file = None;
                #[cfg(unix)]
                std::os::unix::fs::symlink(target, &path)
                    .map_err(|err| NarWriteError::create_symlink_error(path, err))?;
            }
        }
    }
    Ok(())
}

struct CIString(Bytes, String);

impl PartialEq for CIString {
    fn eq(&self, other: &Self) -> bool {
        self.1.eq(&other.1)
    }
}

impl fmt::Display for CIString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let bstr = bstr::BStr::new(&self.0);
        write!(f, "{bstr}")
    }
}

impl Eq for CIString {}

impl std::hash::Hash for CIString {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.1.hash(state)
    }
}

#[derive(Default)]
struct Entries(HashMap<CIString, u32>);

impl Entries {
    fn hack_name(&mut self, name: Bytes) -> Bytes {
        use std::collections::hash_map::Entry;

        let lower = String::from_utf8_lossy(&name).to_lowercase();
        let ci_str = CIString(name.clone(), lower);
        match self.0.entry(ci_str) {
            Entry::Occupied(mut o) => {
                let b_name = bstr::BStr::new(&name);
                debug!("case collision between '{}' and '{}'", o.key(), b_name);
                let idx = o.get() + 1;
                let mut new_name = name.to_vec();
                write!(new_name, "{CASE_HACK_SUFFIX}{idx}").unwrap();
                o.insert(idx);
                Bytes::from(new_name)
            }
            Entry::Vacant(v) => {
                v.insert(0);
                name
            }
        }
    }
}

pub struct RestoreOptions {
    use_case_hack: bool,
}

impl RestoreOptions {
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

    pub async fn restore<S, U, R, P>(self, stream: S, path: P) -> Result<(), NarWriteError>
    where
        S: Stream<Item = U>,
        U: Into<Result<NarEvent<R>, NarWriteError>>,
        P: Into<PathBuf>,
        R: AsyncBufRead + Send + Unpin,
    {
        let restorer = NarRestorer::new_restorer(path, self.use_case_hack);
        restorer.restore(stream).await
    }
}

impl Default for RestoreOptions {
    fn default() -> Self {
        Self::new()
    }
}

pub async fn restore<S, U, R, P>(stream: S, path: P) -> Result<(), NarWriteError>
where
    S: Stream<Item = U>,
    U: Into<Result<NarEvent<R>, NarWriteError>>,
    P: Into<PathBuf>,
    R: AsyncBufRead + Send + Unpin,
{
    RestoreOptions::new().restore(stream, path).await
}

#[cfg(test)]
mod unittests {
    use super::*;
    use crate::archive::{NarEvent, dump, test_data};
    use futures_util::FutureExt as _;
    use futures_util::stream::{StreamExt as _, TryStreamExt as _, iter};
    use rstest::rstest;
    use tempfile::Builder;

    #[tokio::test]
    #[rstest]
    #[case::text_file(test_data::text_file())]
    #[case::exec_file(test_data::exec_file())]
    #[case::empty_file(test_data::empty_file())]
    #[case::empty_file_in_dir(test_data::empty_file_in_dir())]
    #[case::empty_dir(test_data::empty_dir())]
    #[case::empty_dir_in_dir(test_data::empty_dir_in_dir())]
    #[case::symlink(test_data::symlink())]
    #[case::dir_example(test_data::dir_example())]
    #[case::case_hack_sorting(test_data::case_hack_sorting())]
    async fn test_restore(#[case] events: test_data::TestNarEvents) {
        let dir = Builder::new().prefix("test_restore").tempdir().unwrap();
        let path = dir.path().join("output");

        let events_s = iter(events.clone().into_iter())
            .map(|e| Ok(e) as Result<test_data::TestNarEvent, NarWriteError>);
        restore(events_s, &path).await.unwrap();

        let s = dump(path)
            .and_then(NarEvent::read_file)
            .try_collect::<test_data::TestNarEvents>()
            .await
            .unwrap();
        assert_eq!(s, events);
    }

    fn file(name: &'static [u8]) -> test_data::TestNarEvent {
        NarEvent::File {
            name: Bytes::from_static(name),
            executable: false,
            size: 1,
            reader: std::io::Cursor::new(Bytes::from_static(b"x")),
        }
    }

    fn symlink(name: &'static [u8]) -> test_data::TestNarEvent {
        NarEvent::Symlink {
            name: Bytes::from_static(name),
            target: Bytes::from_static(b"/etc"),
        }
    }

    fn start_dir(name: &'static [u8]) -> test_data::TestNarEvent {
        NarEvent::StartDirectory {
            name: Bytes::from_static(name),
        }
    }

    /// `restore` doesn't trust its input: whatever the stream says, nothing
    /// may be created outside the destination.
    #[tokio::test]
    #[rstest]
    #[case::dotdot(vec![start_dir(b""), file(b"..")])]
    #[case::dot(vec![start_dir(b""), file(b".")])]
    #[case::slash(vec![start_dir(b""), file(b"a/b")])]
    #[case::traversal(vec![start_dir(b""), file(b"../escape")])]
    #[case::absolute(vec![start_dir(b""), file(b"/tmp/escape")])]
    #[case::nul(vec![start_dir(b""), file(b"a\0b")])]
    #[case::empty_name(vec![start_dir(b""), file(b"")])]
    #[case::dotdot_dir(vec![start_dir(b""), start_dir(b".."), file(b"escape"), NarEvent::EndDirectory, NarEvent::EndDirectory])]
    #[case::dotdot_symlink(vec![start_dir(b""), symlink(b"../escape")])]
    #[case::named_root(vec![file(b"escape")])]
    #[case::end_without_start(vec![NarEvent::EndDirectory, file(b"escape")])]
    #[case::end_past_root(vec![start_dir(b""), NarEvent::EndDirectory, NarEvent::EndDirectory, file(b"escape")])]
    #[case::after_root_dir(vec![start_dir(b""), NarEvent::EndDirectory, file(b"escape")])]
    #[case::after_root_file(vec![file(b""), file(b"escape")])]
    #[case::second_root(vec![start_dir(b""), NarEvent::EndDirectory, start_dir(b"")])]
    async fn test_restore_rejects_escape(#[case] events: test_data::TestNarEvents) {
        let dir = Builder::new().prefix("test_restore").tempdir().unwrap();
        let path = dir.path().join("output");

        let err = restore(iter(events).map(Ok::<_, NarWriteError>), &path)
            .await
            .unwrap_err();
        assert_eq!(err.operation, NarWriteOperation::InvalidEntry);

        // Only the destination itself may exist next to it.
        let siblings: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .filter(|n| n != "output")
            .collect();
        assert!(siblings.is_empty(), "{siblings:?}");
        assert!(!path.join("..").join("escape").exists());
    }

    /// A symlink is never followed by a later entry of the same name.
    #[tokio::test]
    #[rstest]
    #[case::dir(start_dir(b"link"), "")]
    #[case::file(file(b"link"), "")]
    #[case::dangling_file(file(b"link"), "missing")]
    async fn test_restore_does_not_follow_symlink(
        #[case] second: test_data::TestNarEvent,
        #[case] target_name: &str,
    ) {
        let dir = Builder::new().prefix("test_restore").tempdir().unwrap();
        let outside = dir.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        let path = dir.path().join("output");
        let link = NarEvent::Symlink {
            name: Bytes::from_static(b"link"),
            target: Bytes::copy_from_slice(
                outside.join(target_name).as_os_str().as_encoded_bytes(),
            ),
        };
        let mut events = vec![start_dir(b""), link, second];
        if matches!(events[2], NarEvent::StartDirectory { .. }) {
            events.extend([file(b"escape"), NarEvent::EndDirectory]);
        }
        events.push(NarEvent::EndDirectory);

        restore(iter(events).map(Ok::<_, NarWriteError>), &path)
            .await
            .unwrap_err();
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn test_restore_file_bigger_than_queue() {
        let dir = Builder::new().prefix("test_restore").tempdir().unwrap();
        let path = dir.path().join("output");
        let data: Bytes = (0..2 * DATA_IN_FLIGHT + 12345).map(|i| i as u8).collect();
        let events = vec![NarEvent::File {
            name: Bytes::new(),
            executable: false,
            size: data.len() as u64,
            reader: std::io::Cursor::new(data.clone()),
        }];
        restore(iter(events).map(Ok::<_, NarWriteError>), &path)
            .await
            .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), data);
    }

    #[tokio::test]
    async fn test_restore_errors() {
        let dir = Builder::new().prefix("test_restore").tempdir().unwrap();

        // `restore` returns the writer thread's error even while the stream
        // is still pending.
        let path = dir.path().join("exists");
        std::fs::create_dir(&path).unwrap();
        let events = iter(test_data::dir_example())
            .map(Ok::<_, NarWriteError>)
            .chain(futures_util::stream::pending());
        let restored = restore(events, &path);
        let err = tokio::time::timeout(std::time::Duration::from_secs(10), restored)
            .await
            .expect("`restore` waited on the stream")
            .unwrap_err();
        assert_eq!(err.operation, NarWriteOperation::CreateDirectory);
        assert_eq!(err.path, path);

        // `restore` returns a stream error only after the writer thread
        // finishes the work queued before it.
        let path = dir.path().join("output");
        let failed = NarWriteError::create_file_error("stream".into(), io::ErrorKind::Other.into());
        let events = iter(test_data::dir_example().into_iter().map(Ok))
            .chain(futures_util::stream::once(async { Err(failed) }));
        let err = restore(events, &path).await.unwrap_err();
        assert_eq!(err.path, PathBuf::from("stream"));
        assert!(path.is_dir());
    }

    #[test]
    fn test_restore_dropped() {
        // The runtime has one blocking thread and `busy` holds it, so the
        // writer thread can't start until the test drops the restore future.
        let r = tokio::runtime::Builder::new_current_thread()
            .max_blocking_threads(1)
            .build()
            .unwrap();
        let dir = Builder::new().prefix("test_restore").tempdir().unwrap();
        let path = dir.path().join("output");
        r.block_on(async {
            let (release, wait) = std::sync::mpsc::channel::<()>();
            let busy = spawn_blocking(move || wait.recv().unwrap());

            // `restore` queues the work for the whole NAR and then waits on
            // the stream, so `now_or_never` drops it with all of it queued.
            let events = iter(test_data::dir_example())
                .map(Ok::<_, NarWriteError>)
                .chain(futures_util::stream::pending());
            assert!(restore(events, &path).now_or_never().is_none());

            release.send(()).unwrap();
            busy.await.unwrap();
            // The pool runs blocking tasks in spawn order, so the writer
            // thread has finished by the time this task runs.
            spawn_blocking(|| ()).await.unwrap();
        });
        assert!(!path.exists());
    }

    #[test]
    fn test_restore_data_budget() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        // The runtime has one blocking thread and `busy` holds it, so the
        // writer thread takes nothing from the queue while `restore` runs.
        let r = tokio::runtime::Builder::new_current_thread()
            .max_blocking_threads(1)
            .build()
            .unwrap();
        let dir = Builder::new().prefix("test_restore").tempdir().unwrap();
        let path = dir.path().join("output");
        let read = Arc::new(AtomicUsize::new(0));
        r.block_on(async {
            let (release, wait) = std::sync::mpsc::channel::<()>();
            let busy = spawn_blocking(move || wait.recv().unwrap());

            // The file says it is empty, but its reader returns 8 MiB.
            let data = std::io::Cursor::new(vec![0; 4 * DATA_IN_FLIGHT]);
            let counted = tokio_util::io::InspectReader::new(data, {
                let read = read.clone();
                move |buf: &[u8]| {
                    read.fetch_add(buf.len(), Ordering::Relaxed);
                }
            });
            let events = iter([NarEvent::File {
                name: Bytes::new(),
                executable: false,
                size: 0,
                reader: tokio::io::BufReader::new(counted),
            }]);
            let restored = restore(events.map(Ok::<_, NarWriteError>), &path);
            assert!(restored.now_or_never().is_none());

            release.send(()).unwrap();
            busy.await.unwrap();
            spawn_blocking(|| ()).await.unwrap();
        });
        // `restore` reads one chunk past the budget and then waits for the
        // writer thread. If it trusted `size`, it would read all 8 MiB.
        assert!(read.load(Ordering::Relaxed) < 2 * DATA_IN_FLIGHT);
    }
}

#[cfg(test)]
mod proptests {
    use futures_util::stream::iter;
    use futures_util::{StreamExt as _, TryStreamExt as _};
    use proptest::proptest;
    use tempfile::tempdir;

    use crate::archive::{NarEvent, NarWriteError, dump, restore, test_data};
    use crate::test::arbitrary::archive::arb_nar_events;
    use proptest::prop_assert_eq;

    #[test]
    fn proptest_restore_dump() {
        let r = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        proptest!(|(events in arb_nar_events(8, 256, 10))| {
            r.block_on(async {
                let dir = tempdir()?;
                let path = dir.path().join("output");

                let event_s = iter(events.clone().into_iter())
                    .map(|e| Ok(e) as Result<test_data::TestNarEvent, NarWriteError> );
                restore(event_s, &path).await.unwrap();

                let s = dump(path)
                    .and_then(NarEvent::read_file)
                    .try_collect::<test_data::TestNarEvents>().await?;
                prop_assert_eq!(&s, &events);
                Ok(())
            })?;

        });
    }
}
