use std::collections::HashMap;
use std::ffi::{CStr, OsStr};
use std::fmt;
use std::fs::File;
use std::io::{self, Write as _};
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bytes::Bytes;
use derive_more::Display;
use futures_core::Stream;
use rustix::fs::{
    AtFlags, CWD, Mode, OFlags, Timespec, Timestamps, UTIME_OMIT, chmodat, fchmod, fstat, futimens,
    mkdirat, openat, symlinkat, utimensat,
};
use rustix::io::Errno;
use rustix::path::Arg;
use thiserror::Error;
use tokio::io::{AsyncBufRead, AsyncBufReadExt as _};
use tokio::sync::mpsc;
use tokio::task::{JoinError, spawn_blocking};
use tokio_util::sync::CancellationToken;
use tracing::{debug, trace};

use super::{CASE_HACK_SUFFIX, NarEvent, is_valid_entry_name};

#[derive(Display, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
pub enum NarWriteOperation {
    #[display("creating directory")]
    CreateDirectory,
    #[display("creating symlink")]
    CreateSymlink,
    #[display("creating file")]
    CreateFile,
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
    root: PathBuf,
    /// Open subdirectory names below `root`, kept as owned copies so they do
    /// not pin the parser's read buffer; used only to construct error paths.
    dir_names: Vec<Box<[u8]>>,
    use_case_hack: bool,
    entries: Entries,
    dir_stack: Vec<Entries>,
    /// How many directories of the NAR are open.
    depth: usize,
    /// Whether the NAR's root entry has been seen.
    root_seen: bool,
    canonical: bool,
}

impl NarRestorer {
    pub fn new<P: Into<PathBuf>>(path: P) -> Self {
        Self::new_restorer(path, false, false)
    }

    pub fn with_case_hack<P: Into<PathBuf>>(path: P) -> Self {
        Self::new_restorer(path, true, false)
    }

    fn new_restorer<P>(path: P, use_case_hack: bool, canonical: bool) -> Self
    where
        P: Into<PathBuf>,
    {
        Self {
            root: path.into(),
            dir_names: Vec::new(),
            use_case_hack,
            entries: Default::default(),
            dir_stack: Default::default(),
            depth: 0,
            root_seen: false,
            canonical,
        }
    }

    /// Construct the full path of `leaf` inside the current open directory.
    /// Only called on the cold error path.
    #[cold]
    fn error_path(&self, leaf: &[u8]) -> PathBuf {
        build_error_path(&self.root, self.dir_names.iter().map(Box::as_ref), leaf)
    }

    /// Checks that `event` stays inside the destination.
    ///
    /// The events don't have to come from our parser, so the names and the
    /// nesting are checked here: the root has no name, everything below it has
    /// a single path component, and nothing follows the root or closes it twice.
    fn check_event<R>(&mut self, event: &NarEvent<R>) -> Result<(), NarWriteError> {
        let name = match event {
            NarEvent::File { name, .. }
            | NarEvent::Symlink { name, .. }
            | NarEvent::StartDirectory { name } => name,
            NarEvent::EndDirectory => {
                if self.depth == 0 {
                    return Err(NarWriteError::invalid_entry_error(
                        self.error_path(b""),
                        "end of a directory that was not started",
                    ));
                }
                self.depth -= 1;
                return Ok(());
            }
        };
        if self.depth == 0 {
            if self.root_seen {
                return Err(NarWriteError::invalid_entry_error(
                    self.error_path(name),
                    "entry after the root",
                ));
            }
            if !name.is_empty() {
                return Err(NarWriteError::invalid_entry_error(
                    self.error_path(name),
                    "the root has a name",
                ));
            }
            self.root_seen = true;
        } else if !is_valid_entry_name(name) {
            return Err(NarWriteError::invalid_entry_error(
                self.error_path(name),
                "invalid file name",
            ));
        }
        if matches!(event, NarEvent::StartDirectory { .. }) {
            self.depth += 1;
        }
        Ok(())
    }

    /// Read exactly `want` bytes from `reader` into `dst` using `AsyncBufRead`
    /// directly, avoiding `read_to_end`'s EOF-probe reallocation.
    async fn read_exact_into<R: AsyncBufRead + Unpin>(
        &self,
        reader: &mut R,
        name: &[u8],
        dst: &mut Vec<u8>,
        want: usize,
    ) -> Result<(), NarWriteError> {
        let mut copied = 0usize;
        while copied < want {
            let buf = reader
                .fill_buf()
                .await
                .map_err(|err| NarWriteError::create_file_error(self.error_path(name), err))?;
            if buf.is_empty() {
                return Err(NarWriteError::invalid_entry_error(
                    self.error_path(name),
                    "file size doesn't match",
                ));
            }
            let take = buf.len().min(want - copied);
            dst.extend_from_slice(&buf[..take]);
            reader.consume(take);
            copied += take;
        }
        Ok(())
    }

    /// Verify that `reader` has reached EOF (draining parser padding) without
    /// allocating.
    async fn verify_eof<R: AsyncBufRead + Unpin>(
        &self,
        reader: &mut R,
        name: &[u8],
    ) -> Result<(), NarWriteError> {
        let extra = reader
            .fill_buf()
            .await
            .map_err(|err| NarWriteError::create_file_error(self.error_path(name), err))?;
        if !extra.is_empty() {
            return Err(NarWriteError::invalid_entry_error(
                self.error_path(name),
                "file size doesn't match",
            ));
        }
        Ok(())
    }

    /// Process a single NAR event, appending it to `batch` or flushing to `writer`.
    async fn process_event<R>(
        &mut self,
        event: NarEvent<R>,
        writer: &mut Writer,
        batch: &mut OpBatch,
    ) -> Result<(), NarWriteError>
    where
        R: AsyncBufRead + Unpin,
    {
        use futures_util::FutureExt as _;
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

                // Small files go into the batch's payload when their name and
                // data fit, so a payload never outgrows its reservation.
                if (name.len() as u64 + 1).saturating_add(size) <= BATCH_PAYLOAD_LIMIT as u64 {
                    let want = size as usize;
                    if !batch.ops.is_empty()
                        && (batch.ops.len() >= OPS_PER_BATCH
                            || batch.payload.len() + name.len() + 1 + want > BATCH_PAYLOAD_LIMIT)
                    {
                        writer.flush(batch).await?;
                    }
                    // Copy `name` into the batch buffer and drop the parser's
                    // `Bytes` before reading file data so the parser can reuse
                    // its read buffer.
                    let mut name_span = batch.push_c_name(&name);
                    drop(name);

                    if want > 0 && batch.payload.capacity() < BATCH_PAYLOAD_LIMIT {
                        batch
                            .payload
                            .reserve_exact(BATCH_PAYLOAD_LIMIT - batch.payload.len());
                    }
                    let mut copied = 0usize;
                    // Read the data, then check that the file ends there.
                    loop {
                        let filled = match reader.fill_buf().now_or_never() {
                            Some(filled) => filled,
                            None => {
                                // Before waiting for the stream, hand the
                                // finished entries to the writer thread, as
                                // `restore` does between entries.
                                if !batch.ops.is_empty() {
                                    writer.flush_finished(batch, name_span.offset).await?;
                                    name_span.offset = 0;
                                }
                                reader.fill_buf().await
                            }
                        };
                        let buf = filled.map_err(|err| {
                            NarWriteError::create_file_error(
                                self.error_path(batch.name_bytes(name_span)),
                                err,
                            )
                        })?;
                        let done = copied == want;
                        if buf.is_empty() != done {
                            return Err(NarWriteError::invalid_entry_error(
                                self.error_path(batch.name_bytes(name_span)),
                                "file size doesn't match",
                            ));
                        }
                        if done {
                            break;
                        }
                        let take = buf.len().min(want - copied);
                        batch.payload.extend_from_slice(&buf[..take]);
                        reader.consume(take);
                        copied += take;
                    }
                    batch.ops.push(Op::CreateFileInline {
                        name: name_span,
                        executable,
                        data: Span {
                            offset: name_span.offset + name_span.len + 1,
                            len: want,
                        },
                    });
                    if batch.ops.len() >= OPS_PER_BATCH
                        || batch.payload.len() >= BATCH_PAYLOAD_LIMIT
                    {
                        writer.flush(batch).await?;
                    }
                } else {
                    // Flush buffered metadata/small files before streaming a large file.
                    writer.flush(batch).await?;
                    // Copy the filename out of `name` and drop the parser's
                    // `Bytes` before reading so the parser can reuse its buffer.
                    let owned_name: Box<[u8]> = name.as_ref().into();
                    drop(name);
                    let mut first_name = Some(owned_name.clone());
                    let mut left = size;
                    let mut created = false;
                    // Runs at least once: an empty file whose name is too
                    // long to inline still needs its EOF check and creation.
                    loop {
                        let want = left.min(RESTORE_CHUNK_SIZE as u64) as usize;
                        let mut chunk = writer.take_chunk();
                        self.read_exact_into(&mut reader, &owned_name, &mut chunk, want)
                            .await?;
                        left -= want as u64;
                        // If checking for the end after the last chunk would
                        // wait, the chunk goes first, so the writer thread can
                        // write it (and report errors) meanwhile, and the file
                        // is finished once the check passes.
                        let eof_waits = left == 0
                            && match self.verify_eof(&mut reader, &owned_name).now_or_never() {
                                Some(checked) => {
                                    checked?;
                                    false
                                }
                                None => true,
                            };
                        let finish = left == 0 && !eof_waits;
                        let op = if created {
                            Op::Write {
                                data: chunk,
                                finish,
                            }
                        } else {
                            created = true;
                            Op::CreateFileChunk {
                                name: first_name.take().unwrap(),
                                executable,
                                data: chunk,
                                finish,
                            }
                        };
                        writer.send_msg(Msg::Large(op)).await?;
                        if eof_waits {
                            self.verify_eof(&mut reader, &owned_name).await?;
                            let op = Op::Write {
                                data: Vec::new(),
                                finish: true,
                            };
                            writer.send_msg(Msg::Large(op)).await?;
                        }
                        if left == 0 {
                            break;
                        }
                    }
                }
            }
            NarEvent::Symlink { name, target } => {
                let name = if self.use_case_hack {
                    self.entries.hack_name(name)
                } else {
                    name
                };
                if !batch.ops.is_empty()
                    && batch.payload.len() + name.len() + 1 + target.len() > BATCH_PAYLOAD_LIMIT
                {
                    writer.flush(batch).await?;
                }
                let name_span = batch.push_c_name(&name);
                let target_span = batch.push_raw(&target);
                writer
                    .push_op(
                        batch,
                        Op::CreateSymlink {
                            name: name_span,
                            target: target_span,
                        },
                    )
                    .await?;
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

                if !name.is_empty() {
                    self.dir_names.push(name.as_ref().into());
                }
                if !batch.ops.is_empty()
                    && batch.payload.len() + name.len() + 1 > BATCH_PAYLOAD_LIMIT
                {
                    writer.flush(batch).await?;
                }
                let name_span = batch.push_c_name(&name);
                writer
                    .push_op(batch, Op::CreateDirectory { name: name_span })
                    .await?;
            }
            NarEvent::EndDirectory => {
                if self.use_case_hack {
                    self.entries = self.dir_stack.pop().unwrap_or_default();
                }
                // `check_event` already decremented `depth`, so `depth > 0`
                // means we are leaving a subdirectory below `root`.
                if self.depth > 0 {
                    self.dir_names.pop();
                }
                debug_assert_eq!(self.dir_names.len(), self.depth.saturating_sub(1));
                writer.push_op(batch, Op::EndDirectory).await?;
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
        use futures_util::future::{Either, select};
        use futures_util::{FutureExt as _, StreamExt as _};
        let spare = Spare::default();
        let (ops, queued) = mpsc::channel(BATCHES_IN_FLIGHT);
        let dropped = CancellationToken::new();
        let written = spawn_blocking({
            let dropped = dropped.clone();
            let spare = spare.clone();
            let root = self.root.clone();
            move || write(queued, &dropped, root, self.canonical, spare)
        });
        let writer = Writer {
            ops,
            spare,
            local_chunks: Vec::new(),
            local_batches: Vec::new(),
        };
        let sent = async {
            // This future owns `writer`, so the queue closes when it finishes.
            let mut writer = writer;
            let mut batch = writer.take_batch();
            futures_util::pin_mut!(stream);
            let res: Result<(), NarWriteError> = async {
                loop {
                    // Drain immediately-ready events into the current batch,
                    // flushing before parking when the stream is pending so
                    // disk writes overlap with slow streams and never stall
                    // concurrent filesystem observers.
                    let next_item = if !batch.ops.is_empty() {
                        match stream.next().now_or_never() {
                            Some(item) => item,
                            None => {
                                writer.flush(&mut batch).await?;
                                stream.next().await
                            }
                        }
                    } else {
                        stream.next().await
                    };
                    let Some(item) = next_item else {
                        break;
                    };
                    self.process_event(item.into()?, &mut writer, &mut batch)
                        .await?;
                }
                Ok(())
            }
            .await;
            // Flush queued work before returning even on stream error so prior
            // entries finish (and canonical directories are sealed). Any writer
            // thread error is surfaced via `written` below.
            let flush_res = writer.flush(&mut batch).await;
            res.and(flush_res)
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

/// Returns the path of entry `name` in `dir` with a single allocation.
/// Unlike [`PathBuf::push`], a leading slash in `name` doesn't replace `dir`.
fn join_name(dir: &Path, name: &[u8]) -> PathBuf {
    let mut path = PathBuf::with_capacity(dir.as_os_str().len() + 1 + name.len());
    path.push(dir);
    if !name.is_empty() {
        path.push("");
        path.as_mut_os_string().push(OsStr::from_bytes(name));
    }
    path
}

#[cold]
fn build_error_path<'a>(
    root: &Path,
    dirs: impl IntoIterator<Item = &'a [u8]>,
    leaf: &[u8],
) -> PathBuf {
    let mut path = root.to_path_buf();
    for d in dirs {
        if !d.is_empty() {
            path.push(OsStr::from_bytes(d));
        }
    }
    if !leaf.is_empty() {
        join_name(&path, leaf)
    } else {
        path
    }
}

/// Chunk size for large files in `NarRestorer`, kept at 64 KiB so chunks
/// stay below glibc's 128 KiB `mmap` threshold, align with typical stream
/// chunk sizes and OS pages, and stay L2-cache-hot between producer and writer.
const RESTORE_CHUNK_SIZE: usize = 64 * 1024;

/// Maximum payload bytes accumulated in a single batch before flush. A batch
/// is flushed before an entry that would take it past this, and small files
/// are packed into the payload only if their name and data fit. Matching
/// `RESTORE_CHUNK_SIZE` means every channel message carries at most one
/// chunk's worth of data. Only a directory or symlink whose name or target
/// alone is bigger exceeds it, and the writer thread frees such a payload
/// instead of reusing it.
const BATCH_PAYLOAD_LIMIT: usize = RESTORE_CHUNK_SIZE;

/// How many entries a single batch holds before flushing to the writer thread.
const OPS_PER_BATCH: usize = 64;

/// How many messages the writer thread's channel holds, so the channel holds
/// at most `BATCHES_IN_FLIGHT * RESTORE_CHUNK_SIZE` (512 KiB) of data. That
/// isn't all the data in flight: the writer thread takes up to this many
/// messages out of the channel at once, freeing their slots before it applies
/// them, and `restore` holds the message it waits to send, plus at most one
/// partly read small file. So up to `(2 * BATCHES_IN_FLIGHT + 2) *
/// RESTORE_CHUNK_SIZE` (1.125 MiB) is in flight, apart from the entries that
/// exceed [`BATCH_PAYLOAD_LIMIT`] on their own.
const BATCHES_IN_FLIGHT: usize = 8;

#[cfg(test)]
const DATA_IN_FLIGHT: usize = BATCHES_IN_FLIGHT * RESTORE_CHUNK_SIZE;

/// Byte range inside `OpBatch::payload`.
#[derive(Clone, Copy)]
struct Span {
    offset: usize,
    len: usize,
}

/// Filesystem work for the writer thread, in NAR order.
enum Op {
    /// Directory creation; `name` is a NUL-terminated span in `OpBatch::payload`.
    CreateDirectory {
        name: Span,
    },
    /// Symlink creation; `name` is NUL-terminated, `target` is raw bytes.
    CreateSymlink {
        name: Span,
        target: Span,
    },
    /// Complete small file; `name` is NUL-terminated and `data` holds its bytes.
    CreateFileInline {
        name: Span,
        executable: bool,
        data: Span,
    },
    /// First chunk of a large file.
    CreateFileChunk {
        name: Box<[u8]>,
        executable: bool,
        data: Vec<u8>,
        finish: bool,
    },
    /// Subsequent chunk of the last created large file. An empty one with
    /// `finish` only finishes the file.
    Write {
        data: Vec<u8>,
        finish: bool,
    },
    EndDirectory,
}

struct OpBatch {
    ops: Vec<Op>,
    /// Contiguous storage for NUL-terminated entry names, symlink targets, and
    /// small file payloads in this batch.
    payload: Vec<u8>,
}

impl OpBatch {
    /// Grows the payload for `len` more bytes like `Vec` does, but not past
    /// [`BATCH_PAYLOAD_LIMIT`] while they fit in it, so the writer thread can
    /// reuse the batch.
    #[inline]
    fn make_room(&mut self, len: usize) {
        let need = self.payload.len() + len;
        if need > self.payload.capacity() && need <= BATCH_PAYLOAD_LIMIT {
            let grown = (2 * self.payload.capacity()).clamp(need, BATCH_PAYLOAD_LIMIT);
            self.payload.reserve_exact(grown - self.payload.len());
        }
    }

    /// Append `name` plus a trailing NUL terminator so `Disk` can borrow a
    /// `&CStr` directly with zero conversion. `len` excludes the NUL byte.
    fn push_c_name(&mut self, name: &[u8]) -> Span {
        self.make_room(name.len() + 1);
        let offset = self.payload.len();
        self.payload.extend_from_slice(name);
        self.payload.push(0);
        Span {
            offset,
            len: name.len(),
        }
    }

    fn push_raw(&mut self, bytes: &[u8]) -> Span {
        self.make_room(bytes.len());
        let offset = self.payload.len();
        self.payload.extend_from_slice(bytes);
        Span {
            offset,
            len: bytes.len(),
        }
    }

    #[inline]
    fn name_bytes(&self, span: Span) -> &[u8] {
        &self.payload[span.offset..span.offset + span.len]
    }
}

#[inline]
fn span_bytes(payload: &[u8], span: Span) -> &[u8] {
    &payload[span.offset..span.offset + span.len]
}

#[inline]
fn span_cstr(payload: &[u8], span: Span) -> &CStr {
    CStr::from_bytes_with_nul(&payload[span.offset..span.offset + span.len + 1])
        .expect("validated NAR entry names contain no interior NUL")
}

enum Msg {
    Batch(OpBatch),
    Large(Op),
}

/// Recycled buffers returned by the writer thread so the producer avoids
/// repeated allocations and page faults.
#[derive(Default)]
struct SparePool {
    chunks: Vec<Vec<u8>>,
    batches: Vec<(Vec<Op>, Vec<u8>)>,
}

type Spare = Arc<std::sync::Mutex<SparePool>>;

fn push_capped<T>(dst: &mut Vec<T>, item: T, cap: usize) {
    if dst.len() < cap {
        dst.push(item);
    }
}

fn take_pooled<T>(
    local: &mut Vec<T>,
    spare: &Spare,
    select: impl FnOnce(&mut SparePool) -> &mut Vec<T>,
) -> Option<T> {
    if let Some(item) = local.pop() {
        return Some(item);
    }
    {
        let mut pool = spare.lock().unwrap();
        let shared = select(&mut pool);
        if !shared.is_empty() {
            std::mem::swap(local, shared);
        }
    }
    local.pop()
}

/// The bounded queue to the writer thread.
struct Writer {
    ops: mpsc::Sender<Msg>,
    spare: Spare,
    local_chunks: Vec<Vec<u8>>,
    local_batches: Vec<(Vec<Op>, Vec<u8>)>,
}

impl Writer {
    fn take_chunk(&mut self) -> Vec<u8> {
        take_pooled(&mut self.local_chunks, &self.spare, |p| &mut p.chunks)
            .unwrap_or_else(|| Vec::with_capacity(RESTORE_CHUNK_SIZE))
    }

    fn take_batch(&mut self) -> OpBatch {
        if let Some((ops, payload)) =
            take_pooled(&mut self.local_batches, &self.spare, |p| &mut p.batches)
        {
            return OpBatch { ops, payload };
        }
        OpBatch {
            ops: Vec::with_capacity(OPS_PER_BATCH),
            payload: Vec::new(),
        }
    }

    async fn push_op(&mut self, batch: &mut OpBatch, op: Op) -> Result<(), NarWriteError> {
        batch.ops.push(op);
        if batch.ops.len() >= OPS_PER_BATCH || batch.payload.len() >= BATCH_PAYLOAD_LIMIT {
            self.flush(batch).await?;
        }
        Ok(())
    }

    async fn flush(&mut self, batch: &mut OpBatch) -> Result<(), NarWriteError> {
        if batch.ops.is_empty() {
            return Ok(());
        }
        let to_send = std::mem::replace(batch, self.take_batch());
        self.send_msg(Msg::Batch(to_send)).await
    }

    /// Like [`Writer::flush`], but the payload from `entry` on belongs to an
    /// entry that isn't finished yet, so it moves to the start of the next batch.
    async fn flush_finished(
        &mut self,
        batch: &mut OpBatch,
        entry: usize,
    ) -> Result<(), NarWriteError> {
        let mut next = self.take_batch();
        if next.payload.capacity() < BATCH_PAYLOAD_LIMIT {
            next.payload.reserve_exact(BATCH_PAYLOAD_LIMIT);
        }
        next.payload.extend_from_slice(&batch.payload[entry..]);
        batch.payload.truncate(entry);
        let to_send = std::mem::replace(batch, next);
        self.send_msg(Msg::Batch(to_send)).await
    }

    /// Queues `msg` for the writer thread, waiting while the queue is full.
    ///
    /// This fails only if the writer thread has stopped on an error. `restore`
    /// then returns the thread's error instead of this one.
    async fn send_msg(&self, msg: Msg) -> Result<(), NarWriteError> {
        self.ops.send(msg).await.map_err(|_| {
            NarWriteError::create_file_error(PathBuf::new(), io::ErrorKind::BrokenPipe.into())
        })
    }
}

/// Does the filesystem work in `ops`, in order. If `dropped` is cancelled,
/// it returns before the next entry and skips the rest.
fn write(
    mut ops: mpsc::Receiver<Msg>,
    dropped: &CancellationToken,
    root: PathBuf,
    canonical: bool,
    spare: Spare,
) -> Result<(), NarWriteError> {
    let mut disk = Disk {
        root,
        canonical,
        exact_mode: [None; 2],
        file: None,
        dirs: Vec::new(),
        spare_chunks: Vec::new(),
        spare_batches: Vec::new(),
    };
    let mut msgs = Vec::with_capacity(BATCHES_IN_FLIGHT);
    while ops.blocking_recv_many(&mut msgs, BATCHES_IN_FLIGHT) > 0 {
        let mut res = Ok(());
        for msg in msgs.drain(..) {
            match msg {
                Msg::Batch(mut batch) => {
                    for op in batch.ops.drain(..) {
                        if dropped.is_cancelled() {
                            return Ok(());
                        }
                        if let Err(e) = disk.apply(op, &batch.payload) {
                            res = Err(e);
                            break;
                        }
                    }
                    disk.stash_batch(batch.ops, batch.payload);
                    if res.is_err() {
                        break;
                    }
                }
                Msg::Large(op) => {
                    if dropped.is_cancelled() {
                        return Ok(());
                    }
                    if let Err(e) = disk.apply(op, &[]) {
                        res = Err(e);
                        break;
                    }
                }
            }
        }
        disk.return_spares(&spare);
        res?;
    }
    disk.close_file()
}

const CANONICAL_TIMESTAMPS: Timestamps = Timestamps {
    last_access: Timespec {
        tv_sec: 0,
        tv_nsec: UTIME_OMIT,
    },
    last_modification: Timespec {
        tv_sec: 1,
        tv_nsec: 0,
    },
};

/// Whether files `openat` already gave the canonical mode may skip `fchmod`.
/// Not on FreeBSD: ZFS's default `aclmode=discard` makes chmod also drop
/// inherited ACL entries, which can grant write access the mode doesn't show.
const SKIP_EXACT_FCHMOD: bool = cfg!(any(target_os = "linux", target_os = "macos"));

/// `O_NOFOLLOW` ensures we hold the directory we created, not a symlink that
/// replaced it. `O_PATH` needs no read permission, which Landlock rules that
/// only allow creating entries (like nix-store-csi's) don't grant.
const DIR_FLAGS: OFlags = {
    let flags = OFlags::DIRECTORY
        .union(OFlags::NOFOLLOW)
        .union(OFlags::CLOEXEC);
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let flags = flags.union(OFlags::PATH);
    flags
};

fn set_canonical_mtime(dir: impl AsFd, name: impl Arg) -> Result<(), Errno> {
    utimensat(dir, name, &CANONICAL_TIMESTAMPS, AtFlags::SYMLINK_NOFOLLOW)
}

struct Disk {
    root: PathBuf,
    canonical: bool,
    /// Whether `openat` yields the canonical mode, indexed by `executable`.
    exact_mode: [Option<bool>; 2],
    /// Open multi-chunk file, which [`Op::Write`] appends to.
    file: Option<(Box<[u8]>, File)>,
    /// Open directories, innermost last. Entries are created relative to
    /// the innermost one, so swapping a directory for a symlink can't
    /// redirect them.
    dirs: Vec<(Box<[u8]>, OwnedFd)>,
    spare_chunks: Vec<Vec<u8>>,
    spare_batches: Vec<(Vec<Op>, Vec<u8>)>,
}

impl Disk {
    #[cold]
    fn error_path(&self, leaf: &[u8]) -> PathBuf {
        build_error_path(&self.root, self.dirs.iter().map(|(d, _)| d.as_ref()), leaf)
    }

    fn create_file_at(
        &mut self,
        entry_name: impl Arg,
        err_leaf: &[u8],
        executable: bool,
    ) -> Result<File, NarWriteError> {
        trace!("Writing to file {:?}", OsStr::from_bytes(err_leaf));
        let mut mode = if executable { 0o777 } else { 0o666 };
        if self.canonical {
            // The mode only applies to later opens.
            mode &= !0o222;
        }
        let mode = Mode::from_raw_mode(mode);
        // `O_EXCL` also refuses to follow a symlink.
        let flags = OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC;
        let opened = match self.dirs.last() {
            Some((_, dir)) => openat(dir, entry_name, flags, mode),
            None => openat(CWD, self.root.as_os_str(), flags, mode),
        };
        let created = opened.map_err(|err| {
            NarWriteError::create_file_error(self.error_path(err_leaf), err.into())
        })?;
        if self.canonical {
            // The umask or a default ACL can take bits off `mode`.
            // We create every directory, so all share one default
            // ACL and the first file of each kind decides.
            let exact = self.exact_mode[usize::from(executable)].get_or_insert_with(|| {
                SKIP_EXACT_FCHMOD
                    && fstat(&created).is_ok_and(|st| Mode::from_raw_mode(st.st_mode) == mode)
            });
            if !*exact {
                fchmod(&created, mode).map_err(|err| {
                    NarWriteError::create_file_error(self.error_path(err_leaf), err.into())
                })?;
            }
        }
        Ok(File::from(created))
    }

    fn finish_file(&self, file: &File, err_leaf: &[u8]) -> Result<(), NarWriteError> {
        if self.canonical {
            futimens(file, &CANONICAL_TIMESTAMPS).map_err(|err| {
                NarWriteError::create_file_error(self.error_path(err_leaf), err.into())
            })?;
        }
        Ok(())
    }

    fn apply(&mut self, op: Op, payload: &[u8]) -> Result<(), NarWriteError> {
        match op {
            Op::CreateFileInline {
                name,
                executable,
                data,
            } => {
                self.close_file()?;
                let leaf = span_bytes(payload, name);
                let c_name = span_cstr(payload, name);
                let mut file = self.create_file_at(c_name, leaf, executable)?;
                if data.len > 0 {
                    file.write_all(span_bytes(payload, data)).map_err(|err| {
                        NarWriteError::create_file_error(self.error_path(leaf), err)
                    })?;
                }
                self.finish_file(&file, leaf)?;
            }
            Op::CreateFileChunk {
                name,
                executable,
                data,
                finish,
            } => {
                self.close_file()?;
                let mut file = self.create_file_at(OsStr::from_bytes(&name), &name, executable)?;
                let write_res = file.write_all(&data);
                self.stash_chunk(data);
                write_res
                    .map_err(|err| NarWriteError::create_file_error(self.error_path(&name), err))?;
                if finish {
                    self.finish_file(&file, &name)?;
                } else {
                    self.file = Some((name, file));
                }
            }
            Op::Write { data, finish } => {
                let write_res = {
                    let (_, file) = self.file.as_mut().expect("file data follows its file");
                    file.write_all(&data)
                };
                self.stash_chunk(data);
                if let Err(err) = write_res {
                    let (name, _) = self.file.take().unwrap();
                    return Err(NarWriteError::create_file_error(
                        self.error_path(&name),
                        err,
                    ));
                }
                if finish {
                    self.close_file()?;
                }
            }
            Op::CreateDirectory { name } => {
                self.close_file()?;
                let leaf = span_bytes(payload, name);
                let opened = match self.dirs.last() {
                    Some((_, dir)) => {
                        let c_name = span_cstr(payload, name);
                        mkdirat(dir, c_name, Mode::from_raw_mode(0o777))
                            .and_then(|()| openat(dir, c_name, DIR_FLAGS, Mode::empty()))
                    }
                    None => self.root.as_os_str().into_with_c_str(|c_name| {
                        mkdirat(CWD, c_name, Mode::from_raw_mode(0o777))?;
                        openat(CWD, c_name, DIR_FLAGS, Mode::empty())
                    }),
                }
                .map_err(|err| {
                    NarWriteError::create_dir_error(self.error_path(leaf), err.into())
                })?;
                self.dirs.push((leaf.into(), opened));
            }
            Op::CreateSymlink { name, target } => {
                self.close_file()?;
                let leaf = span_bytes(payload, name);
                let target_bytes = span_bytes(payload, target);
                match self.dirs.last() {
                    Some((_, dir)) => {
                        let c_name = span_cstr(payload, name);
                        symlinkat(target_bytes, dir, c_name).and_then(|()| {
                            if self.canonical {
                                set_canonical_mtime(dir, c_name)?;
                            }
                            Ok(())
                        })
                    }
                    None => self.root.as_os_str().into_with_c_str(|c_name| {
                        symlinkat(target_bytes, CWD, c_name)?;
                        if self.canonical {
                            set_canonical_mtime(CWD, c_name)?;
                        }
                        Ok(())
                    }),
                }
                .map_err(|err| {
                    NarWriteError::create_symlink_error(self.error_path(leaf), err.into())
                })?;
            }
            Op::EndDirectory => {
                self.close_file()?;
                let (name, dir) = self.dirs.pop().expect("a directory ends after it starts");
                if self.canonical {
                    // `fchmod` fails on an `O_PATH` descriptor, but "." works.
                    chmodat(&dir, c".", Mode::from_raw_mode(0o555), AtFlags::empty())
                        .and_then(|()| set_canonical_mtime(&dir, c"."))
                        .map_err(|err| {
                            NarWriteError::create_dir_error(self.error_path(&name), err.into())
                        })?;
                }
            }
        }
        Ok(())
    }

    fn stash_chunk(&mut self, mut data: Vec<u8>) {
        if data.capacity() == RESTORE_CHUNK_SIZE {
            data.clear();
            push_capped(&mut self.spare_chunks, data, BATCHES_IN_FLIGHT);
        }
    }

    fn stash_batch(&mut self, mut ops: Vec<Op>, mut payload: Vec<u8>) {
        if ops.capacity() >= OPS_PER_BATCH && payload.capacity() <= BATCH_PAYLOAD_LIMIT {
            ops.clear();
            payload.clear();
            push_capped(&mut self.spare_batches, (ops, payload), BATCHES_IN_FLIGHT);
        }
    }

    fn return_spares(&mut self, spare: &Spare) {
        if self.spare_chunks.is_empty() && self.spare_batches.is_empty() {
            return;
        }
        let mut pool = spare.lock().unwrap();
        for buf in self.spare_chunks.drain(..) {
            push_capped(&mut pool.chunks, buf, BATCHES_IN_FLIGHT);
        }
        for batch in self.spare_batches.drain(..) {
            push_capped(&mut pool.batches, batch, BATCHES_IN_FLIGHT);
        }
    }

    fn close_file(&mut self) -> Result<(), NarWriteError> {
        if let Some((name, file)) = self.file.take() {
            self.finish_file(&file, &name)?;
        }
        Ok(())
    }
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
    canonical: bool,
}

impl RestoreOptions {
    pub fn new() -> Self {
        #[cfg(target_os = "macos")]
        let use_case_hack = true;
        #[cfg(not(target_os = "macos"))]
        let use_case_hack = false;
        Self {
            use_case_hack,
            canonical: false,
        }
    }

    pub fn use_case_hack(mut self, use_case_hack: bool) -> Self {
        self.use_case_hack = use_case_hack;
        self
    }

    /// Give entries the metadata of store paths while restoring: mode
    /// 0o444/0o555 and mtime 1. The mode doesn't depend on the umask, as
    /// long as the umask doesn't change during the restore.
    pub fn canonical(mut self, canonical: bool) -> Self {
        self.canonical = canonical;
        self
    }

    pub async fn restore<S, U, R, P>(self, stream: S, path: P) -> Result<(), NarWriteError>
    where
        S: Stream<Item = U>,
        U: Into<Result<NarEvent<R>, NarWriteError>>,
        P: Into<PathBuf>,
        R: AsyncBufRead + Send + Unpin,
    {
        NarRestorer::new_restorer(path, self.use_case_hack, self.canonical)
            .restore(stream)
            .await
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
    use std::fs::Permissions;
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;
    use crate::archive::{NarEvent, dump, test_data};
    use futures_util::FutureExt as _;
    use futures_util::stream::{StreamExt as _, TryStreamExt as _, iter};
    use rstest::rstest;
    use tempfile::Builder;

    /// The umask is per process, and tests run in parallel threads.
    static UMASK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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
    #[case::many_files(many_files())]
    async fn test_restore(
        #[case] events: test_data::TestNarEvents,
        #[values(false, true)] canonical: bool,
    ) {
        let _umask = UMASK.lock().await;
        let dir = Builder::new().prefix("test_restore").tempdir().unwrap();
        let path = dir.path().join("output");

        let events_s = iter(events.clone().into_iter())
            .map(|e| Ok(e) as Result<test_data::TestNarEvent, NarWriteError>);
        RestoreOptions::new()
            .canonical(canonical)
            .restore(events_s, &path)
            .await
            .unwrap();

        let s = dump(&path)
            .and_then(NarEvent::read_file)
            .try_collect::<test_data::TestNarEvents>()
            .await
            .unwrap();
        assert_eq!(s, events);
        if canonical {
            check_canonical(&path);
        }
    }

    /// Also makes directories writable again so the tempdir can be removed.
    fn check_canonical(path: &Path) {
        use std::os::unix::fs::MetadataExt as _;

        for entry in walkdir::WalkDir::new(path).follow_root_links(false) {
            let entry = entry.unwrap();
            let meta = entry.metadata().unwrap();
            let at = entry.path();
            assert_eq!((meta.mtime(), meta.mtime_nsec()), (1, 0), "{at:?}");
            let mode = meta.mode() & 0o7777;
            if meta.is_dir() {
                assert_eq!(mode, 0o555, "{at:?}");
                std::fs::set_permissions(at, Permissions::from_mode(0o755)).unwrap();
            } else if meta.is_file() {
                assert!(matches!(mode, 0o444 | 0o555), "{at:?}: {mode:o}");
            }
        }
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

    /// Events for more files than the writer thread's queue and one batch
    /// hold. Each file contains its own name, so data written to the wrong
    /// file fails the test.
    fn many_files() -> test_data::TestNarEvents {
        let files = (0..2 * BATCHES_IN_FLIGHT * OPS_PER_BATCH).map(|i| {
            let name = Bytes::from(format!("{i:04}"));
            NarEvent::File {
                name: name.clone(),
                executable: false,
                size: name.len() as u64,
                reader: std::io::Cursor::new(name),
            }
        });
        std::iter::once(start_dir(b""))
            .chain(files)
            .chain([NarEvent::EndDirectory])
            .collect()
    }

    /// `restore` doesn't trust its input: whatever the stream says, nothing
    /// may be created outside the destination.
    #[tokio::test]
    #[rstest]
    #[case::dotdot(vec![start_dir(b""), file(b"..")], "output/..")]
    #[case::dot(vec![start_dir(b""), file(b".")], "output/.")]
    #[case::slash(vec![start_dir(b""), file(b"a/b")], "output/a/b")]
    #[case::traversal(vec![start_dir(b""), file(b"../escape")], "output/../escape")]
    #[case::absolute(vec![start_dir(b""), file(b"/tmp/escape")], "output//tmp/escape")]
    #[case::nul(vec![start_dir(b""), file(b"a\0b")], "output/a\0b")]
    #[case::empty_name(vec![start_dir(b""), file(b"")], "output")]
    #[case::dotdot_dir(vec![start_dir(b""), start_dir(b".."), file(b"escape"), NarEvent::EndDirectory, NarEvent::EndDirectory], "output/..")]
    #[case::dotdot_symlink(vec![start_dir(b""), symlink(b"../escape")], "output/../escape")]
    #[case::named_root(vec![file(b"escape")], "output/escape")]
    #[case::end_without_start(vec![NarEvent::EndDirectory, file(b"escape")], "output")]
    #[case::end_past_root(vec![start_dir(b""), NarEvent::EndDirectory, NarEvent::EndDirectory, file(b"escape")], "output")]
    #[case::after_root_dir(vec![start_dir(b""), NarEvent::EndDirectory, file(b"escape")], "output/escape")]
    #[case::after_root_file(vec![file(b""), file(b"escape")], "output/escape")]
    #[case::second_root(vec![start_dir(b""), NarEvent::EndDirectory, start_dir(b"")], "output")]
    #[case::after_nested_dir(vec![start_dir(b""), start_dir(b"a"), NarEvent::EndDirectory, file(b"..")], "output/..")]
    async fn test_restore_rejects_escape(
        #[case] events: test_data::TestNarEvents,
        #[case] reported: &str,
    ) {
        let dir = Builder::new().prefix("test_restore").tempdir().unwrap();
        let path = dir.path().join("output");

        let err = restore(iter(events).map(Ok::<_, NarWriteError>), &path)
            .await
            .unwrap_err();
        assert_eq!(err.operation, NarWriteOperation::InvalidEntry);
        assert_eq!(err.path.as_os_str(), dir.path().join(reported).as_os_str());

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

        // The case hack, on by default on macOS, would rename the second
        // entry instead of letting it collide with the symlink.
        RestoreOptions::new()
            .use_case_hack(false)
            .restore(iter(events).map(Ok::<_, NarWriteError>), &path)
            .await
            .unwrap_err();
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
    }

    /// Swapping a restored directory for a symlink mustn't redirect writes.
    #[tokio::test]
    #[rstest]
    async fn test_restore_keeps_its_directories(#[values(false, true)] canonical: bool) {
        let dir = Builder::new().prefix("test_restore").tempdir().unwrap();
        let outside = dir.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        let mode = std::fs::metadata(&outside).unwrap().permissions().mode();
        let path = dir.path().join("output");
        let swap = {
            let (sub, outside) = (path.join("sub"), outside.clone());
            async move {
                while !sub.exists() {
                    tokio::task::yield_now().await;
                }
                std::fs::rename(&sub, sub.with_file_name("moved")).unwrap();
                std::os::unix::fs::symlink(outside, &sub).unwrap();
                None
            }
        };
        let events = iter([start_dir(b""), start_dir(b"sub")])
            .chain(futures_util::stream::once(swap).filter_map(std::future::ready))
            .chain(iter([
                file(b"f"),
                NarEvent::EndDirectory,
                NarEvent::EndDirectory,
            ]));

        // Restore may fail, depending on when the swap happens.
        let _ = RestoreOptions::new()
            .canonical(canonical)
            .restore(events.map(Ok::<_, NarWriteError>), &path)
            .await;
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
        let after = std::fs::metadata(&outside).unwrap().permissions().mode();
        assert_eq!(after, mode);

        for entry in walkdir::WalkDir::new(&path) {
            let entry = entry.unwrap();
            if entry.file_type().is_dir() {
                std::fs::set_permissions(entry.path(), Permissions::from_mode(0o755)).unwrap();
            }
        }
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
    #[rstest]
    #[case::longer(1, 2)]
    #[case::shorter(2, 1)]
    #[case::not_empty(0, 1)]
    // The biggest file named `f` that goes into a batch's payload.
    #[case::longer_inline(BATCH_PAYLOAD_LIMIT - 2, BATCH_PAYLOAD_LIMIT - 1)]
    #[case::longer_than_chunk(RESTORE_CHUNK_SIZE, RESTORE_CHUNK_SIZE + 1)]
    #[case::longer_by_chunk(RESTORE_CHUNK_SIZE + 1, RESTORE_CHUNK_SIZE + 2)]
    #[case::shorter_by_chunk(RESTORE_CHUNK_SIZE + 1, RESTORE_CHUNK_SIZE)]
    async fn test_restore_checks_file_size(#[case] size: usize, #[case] len: usize) {
        let dir = Builder::new().prefix("test_restore").tempdir().unwrap();
        let path = dir.path().join("output");
        let events = [
            start_dir(b""),
            NarEvent::File {
                name: Bytes::from_static(b"f"),
                executable: false,
                size: size as u64,
                reader: std::io::Cursor::new(Bytes::from(vec![0; len])),
            },
            NarEvent::EndDirectory,
        ];
        let err = restore(iter(events).map(Ok::<_, NarWriteError>), &path)
            .await
            .unwrap_err();
        assert_eq!(err.operation, NarWriteOperation::InvalidEntry);
        assert_eq!(err.path, path.join("f"));
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

    /// A file's reader that has `first`, waits once, and then has `rest`.
    fn waiting_reader(
        first: impl Into<Bytes>,
        rest: impl Into<Bytes>,
    ) -> impl AsyncBufRead + Send + Unpin {
        let rest = rest.into();
        let rest = futures_util::stream::once(Box::pin(async move {
            tokio::task::yield_now().await;
            Ok::<_, io::Error>(rest)
        }));
        tokio_util::io::StreamReader::new(iter([Ok(first.into())]).chain(rest))
    }

    /// The entries before a small file reach the writer thread before
    /// `restore` waits for the file's data, so its errors don't wait for a
    /// stalled stream either.
    #[tokio::test]
    async fn test_restore_errors_while_file_waits() {
        let dir = Builder::new().prefix("test_restore").tempdir().unwrap();
        let path = dir.path().join("exists");
        std::fs::create_dir(&path).unwrap();
        let stalled = tokio_util::io::StreamReader::new(
            iter([Ok(Bytes::from_static(b"x"))])
                .chain(futures_util::stream::pending::<io::Result<Bytes>>()),
        );
        let events = [
            NarEvent::StartDirectory { name: Bytes::new() },
            NarEvent::File {
                name: Bytes::from_static(b"f"),
                executable: false,
                size: 2,
                reader: stalled,
            },
        ];
        let restored = restore(iter(events).map(Ok::<_, NarWriteError>), &path);
        let err = tokio::time::timeout(std::time::Duration::from_secs(10), restored)
            .await
            .expect("`restore` waited on the file")
            .unwrap_err();
        assert_eq!(err.operation, NarWriteOperation::CreateDirectory);
        assert_eq!(err.path, path);
    }

    /// A streamed file's last chunk reaches the writer thread before `restore`
    /// waits for the file's end, so its errors don't wait for a stalled
    /// stream either.
    #[tokio::test]
    async fn test_restore_errors_while_file_end_waits() {
        let dir = Builder::new().prefix("test_restore").tempdir().unwrap();
        let path = dir.path().join("exists");
        std::fs::write(&path, b"").unwrap();
        let stalled = tokio_util::io::StreamReader::new(
            iter([Ok(Bytes::from(vec![0; RESTORE_CHUNK_SIZE]))])
                .chain(futures_util::stream::pending::<io::Result<Bytes>>()),
        );
        let events = [NarEvent::File {
            name: Bytes::new(),
            executable: false,
            size: RESTORE_CHUNK_SIZE as u64,
            reader: stalled,
        }];
        let restored = restore(iter(events).map(Ok::<_, NarWriteError>), &path);
        let err = tokio::time::timeout(std::time::Duration::from_secs(10), restored)
            .await
            .expect("`restore` waited on the file's end")
            .unwrap_err();
        assert_eq!(err.operation, NarWriteOperation::CreateFile);
        assert_eq!(err.path, path);
    }

    /// While `restore` waits for a streamed file's end, the writer thread
    /// already has all of its data, like the merge base had for files of
    /// whole chunks.
    #[tokio::test]
    async fn test_restore_writes_file_before_end_waits() {
        let dir = Builder::new().prefix("test_restore").tempdir().unwrap();
        let path = dir.path().join("output");
        let size = 4 * RESTORE_CHUNK_SIZE;
        let stalled = tokio_util::io::StreamReader::new(
            iter([Ok(Bytes::from(vec![1; size]))])
                .chain(futures_util::stream::pending::<io::Result<Bytes>>()),
        );
        let events = [NarEvent::File {
            name: Bytes::new(),
            executable: false,
            size: size as u64,
            reader: stalled,
        }];
        let restored = tokio::spawn(restore(
            iter(events).map(Ok::<_, NarWriteError>),
            path.clone(),
        ));
        let written = async {
            while std::fs::metadata(&path).map_or(0, |m| m.len()) < size as u64 {
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            }
        };
        tokio::time::timeout(std::time::Duration::from_secs(10), written)
            .await
            .expect("the last chunk waited for the file's end");
        restored.abort();
    }

    /// A streamed file whose end comes after a wait is finished like any other.
    #[tokio::test]
    #[rstest]
    #[case::one_chunk(RESTORE_CHUNK_SIZE)]
    #[case::partial_last_chunk(2 * RESTORE_CHUNK_SIZE + 5)]
    async fn test_restore_streamed_file_end_waits(
        #[case] size: usize,
        #[values(false, true)] canonical: bool,
    ) {
        let _umask = UMASK.lock().await;
        let dir = Builder::new().prefix("test_restore").tempdir().unwrap();
        let path = dir.path().join("output");
        let data: Bytes = (0..size).map(|i| i as u8).collect();
        let events = [NarEvent::File {
            name: Bytes::new(),
            executable: true,
            size: size as u64,
            reader: waiting_reader(data.clone(), Bytes::new()),
        }];
        RestoreOptions::new()
            .canonical(canonical)
            .restore(iter(events).map(Ok::<_, NarWriteError>), &path)
            .await
            .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), data);
        if canonical {
            check_canonical(&path);
        }
    }

    /// Small files whose data arrives in parts restore like any other, also
    /// when the entries before them are sent ahead while they wait.
    #[tokio::test]
    async fn test_restore_files_that_wait() {
        let dir = Builder::new().prefix("test_restore").tempdir().unwrap();
        let path = dir.path().join("output");
        let file =
            |name: &'static [u8], first: &'static [u8], rest: &'static [u8]| NarEvent::File {
                name: Bytes::from_static(name),
                executable: false,
                size: (first.len() + rest.len()) as u64,
                reader: waiting_reader(first, rest),
            };
        let events = [
            NarEvent::StartDirectory { name: Bytes::new() },
            file(b"a", b"ab", b"cd"),
            file(b"b", b"", b"efgh"),
            file(b"c", b"ijkl", b""),
            NarEvent::EndDirectory,
        ];
        restore(iter(events).map(Ok::<_, NarWriteError>), &path)
            .await
            .unwrap();
        for (name, data) in [("a", "abcd"), ("b", "efgh"), ("c", "ijkl")] {
            assert_eq!(std::fs::read_to_string(path.join(name)).unwrap(), data);
        }
    }

    /// A batch's payload stays within [`BATCH_PAYLOAD_LIMIT`] unless it holds
    /// a single entry bigger than that, and only payloads within the limit are
    /// reused.
    #[tokio::test]
    async fn test_restore_payload_limit() {
        let (ops, mut queued) = mpsc::channel(64);
        let mut writer = Writer {
            ops,
            spare: Spare::default(),
            local_chunks: Vec::new(),
            local_batches: Vec::new(),
        };
        let mut batch = writer.take_batch();
        let mut restorer = NarRestorer::new("output");
        let file = |name: Bytes, size: usize| NarEvent::File {
            name,
            executable: false,
            size: size as u64,
            reader: std::io::Cursor::new(Bytes::from(vec![0; size])),
        };
        let long = |c: u8, len: usize| Bytes::from(vec![c; len]);
        let mut events = vec![
            NarEvent::StartDirectory { name: Bytes::new() },
            // Leaves too little room for the next directory's name.
            file(long(b'a', 1), BATCH_PAYLOAD_LIMIT - 100),
            NarEvent::StartDirectory {
                name: long(b'b', 255),
            },
            // Fits a payload exactly.
            file(long(b'c', 200), BATCH_PAYLOAD_LIMIT - 201),
            // Fits with a shorter name only.
            file(long(b'd', 200), BATCH_PAYLOAD_LIMIT - 200),
            file(long(b'e', 1), BATCH_PAYLOAD_LIMIT),
            NarEvent::EndDirectory,
            NarEvent::Symlink {
                name: long(b'f', 1),
                target: long(b'/', 2 * BATCH_PAYLOAD_LIMIT),
            },
        ];
        // A fresh batch that comes close to the limit, which doubling its
        // capacity alone would take past it.
        events.extend((0..17).map(|i| NarEvent::Symlink {
            name: Bytes::from(format!("s{i:02}")),
            target: long(b'/', 3000),
        }));
        events.push(NarEvent::EndDirectory);
        for event in events {
            restorer
                .process_event(event, &mut writer, &mut batch)
                .await
                .unwrap();
        }
        writer.flush(&mut batch).await.unwrap();
        drop(writer);

        let mut disk = Disk {
            root: PathBuf::new(),
            canonical: false,
            exact_mode: [None; 2],
            file: None,
            dirs: Vec::new(),
            spare_chunks: Vec::new(),
            spare_batches: Vec::new(),
        };
        let (mut batches, mut oversized, mut symlinks) = (0, 0, 0);
        while let Some(msg) = queued.recv().await {
            let Msg::Batch(batch) = msg else { continue };
            batches += 1;
            if batch.payload.capacity() > BATCH_PAYLOAD_LIMIT {
                assert!(matches!(batch.ops[..], [Op::CreateSymlink { .. }]));
                oversized += 1;
            } else if batch.ops.len() > 1 && matches!(batch.ops[0], Op::CreateSymlink { .. }) {
                assert!(batch.payload.len() > BATCH_PAYLOAD_LIMIT * 3 / 4);
                symlinks += 1;
            }
            disk.stash_batch(batch.ops, batch.payload);
        }
        assert_eq!((oversized, symlinks), (1, 1));
        assert_eq!(disk.spare_batches.len(), batches - oversized);
        for (_, payload) in &disk.spare_batches {
            assert!(payload.capacity() <= BATCH_PAYLOAD_LIMIT);
        }
    }

    /// An empty file whose name is too long to inline is still checked and
    /// created, which fails for a name that long.
    #[tokio::test]
    #[rstest]
    #[case::longer(b"x", NarWriteOperation::InvalidEntry)]
    #[case::empty(b"", NarWriteOperation::CreateFile)]
    async fn test_restore_empty_file_with_long_name(
        #[case] data: &'static [u8],
        #[case] operation: NarWriteOperation,
    ) {
        let dir = Builder::new().prefix("test_restore").tempdir().unwrap();
        let path = dir.path().join("output");
        let name = Bytes::from(vec![b'n'; BATCH_PAYLOAD_LIMIT]);
        let events = [
            start_dir(b""),
            NarEvent::File {
                name: name.clone(),
                executable: false,
                size: 0,
                reader: std::io::Cursor::new(Bytes::from_static(data)),
            },
            NarEvent::EndDirectory,
        ];
        let err = RestoreOptions::new()
            .use_case_hack(false)
            .restore(iter(events).map(Ok::<_, NarWriteError>), &path)
            .await
            .unwrap_err();
        assert_eq!(err.operation, operation);
        assert_eq!(err.path, join_name(&path, &name));
    }

    /// Each case's name says which files the umask takes bits off.
    #[tokio::test]
    #[rstest]
    #[case::none(0o022)]
    #[case::executables(0o011)]
    #[case::all(0o077)]
    async fn test_restore_canonical_umask(#[case] mask: rustix::fs::RawMode) {
        use rustix::process::umask;

        struct Restore(Mode);
        impl Drop for Restore {
            fn drop(&mut self) {
                umask(self.0);
            }
        }

        let _umask = UMASK.lock().await;
        let dir = Builder::new().prefix("test_restore").tempdir().unwrap();
        let path = dir.path().join("output");
        // Put a plain file before the first executable, so the 011 case fails
        // if both kinds share one check.
        let mut events = test_data::dir_example();
        events.insert(1, file(b"a"));
        let old = Restore(umask(Mode::from_raw_mode(mask)));
        let restored = RestoreOptions::new()
            .canonical(true)
            .restore(iter(events.clone()).map(Ok::<_, NarWriteError>), &path)
            .await;
        drop(old);
        restored.unwrap();

        let dumped = dump(&path)
            .and_then(NarEvent::read_file)
            .try_collect::<test_data::TestNarEvents>()
            .await
            .unwrap();
        assert_eq!(dumped, events);
        check_canonical(&path);
    }

    /// With `canonical`, a directory that ended is read-only even if the
    /// restore fails before the next entry.
    #[tokio::test]
    async fn test_restore_canonical_after_error() {
        let dir = Builder::new().prefix("test_restore").tempdir().unwrap();
        let path = dir.path().join("output");
        let failed = NarWriteError::create_file_error("stream".into(), io::ErrorKind::Other.into());
        let events = [
            start_dir(b""),
            start_dir(b"done"),
            file(b"f"),
            NarEvent::EndDirectory,
        ];
        let events = iter(events.map(Ok)).chain(futures_util::stream::once(async { Err(failed) }));
        let err = RestoreOptions::new()
            .canonical(true)
            .restore(events, &path)
            .await
            .unwrap_err();
        assert_eq!(err.path, PathBuf::from("stream"));

        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode();
        assert_eq!(mode(&path.join("done")) & 0o777, 0o555);
        assert_ne!(
            mode(&path) & 0o200,
            0,
            "the root is read-only before it ended"
        );
        std::fs::set_permissions(path.join("done"), Permissions::from_mode(0o755)).unwrap();
    }

    /// A read error names the file being read. For the root, that is the
    /// destination itself, without a trailing slash.
    #[tokio::test]
    async fn test_restore_read_error_path() {
        let dir = Builder::new().prefix("test_restore").tempdir().unwrap();
        let broken = |name: &'static [u8]| NarEvent::File {
            name: Bytes::from_static(name),
            executable: false,
            size: 2,
            reader: tokio_util::io::StreamReader::new(iter([
                Ok(Bytes::from_static(b"x")),
                Err(io::Error::other("broken")),
            ])),
        };

        let path = dir.path().join("root");
        let events = iter([broken(b"")]).map(Ok::<_, NarWriteError>);
        let err = restore(events, &path).await.unwrap_err();
        assert_eq!(err.operation, NarWriteOperation::CreateFile);
        assert_eq!(err.path.as_os_str(), path.as_os_str());

        let path = dir.path().join("nested");
        let events = [
            NarEvent::StartDirectory { name: Bytes::new() },
            broken(b"f"),
        ];
        let err = restore(iter(events).map(Ok::<_, NarWriteError>), &path)
            .await
            .unwrap_err();
        assert_eq!(err.operation, NarWriteOperation::CreateFile);
        assert_eq!(err.path.as_os_str(), path.join("f").as_os_str());
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
                size: 4 * DATA_IN_FLIGHT as u64,
                reader: tokio::io::BufReader::new(counted),
            }]);
            let restored = restore(events.map(Ok::<_, NarWriteError>), &path);
            assert!(restored.now_or_never().is_none());

            release.send(()).unwrap();
            busy.await.unwrap();
            spawn_blocking(|| ()).await.unwrap();
        });
        // `restore` reads one chunk past the budget and then waits for the
        // writer thread instead of reading all 8 MiB.
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
