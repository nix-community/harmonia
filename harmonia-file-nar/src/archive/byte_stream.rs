use std::collections::VecDeque;
use std::future::Future as _;
use std::io;
use std::os::fd::{AsFd, OwnedFd};
use std::path::PathBuf;
use std::pin::Pin;
use std::task::{Context, Poll, ready};

use bytes::Bytes;
use futures_core::Stream;
use tokio::task::{JoinHandle, spawn_blocking};

use super::dumper::{
    DirWalker, SMALL_FILE_THRESHOLD, STREAM_FILE_CHUNK_SIZE, WalkEvent, read_chunk_uninit,
    read_whole_file_uninit,
};
use super::read_nar::{
    TOK_DIR, TOK_ENTRY, TOK_FILE, TOK_FILE_E, TOK_NODE, TOK_PAR, TOK_ROOT, TOK_SYM,
};
use crate::wire::{ZEROS, calc_padding};

/// Flush accumulated framing + small files into an emitted `Bytes` chunk once it
/// reaches this size.
const FRAME_FLUSH_THRESHOLD: usize = 64 * 1024;
/// Files up to this size are read directly into the active framing buffer with
/// no intermediate buffer allocation or copy.
const INLINE_FILE_LIMIT: usize = 56 * 1024;
/// Initial capacity of a framing chunk (kept below 128 KiB so allocations stay
/// in the heap cache rather than triggering `mmap`/`munmap` page faults).
const FRAME_CHUNK_CAPACITY: usize = FRAME_FLUSH_THRESHOLD + INLINE_FILE_LIMIT + 256;
/// Target encoded NAR bytes produced per `spawn_blocking` batch so emitted
/// chunks stay L2-cache-hot for immediate hashing/streaming while avoiding
/// frequent thread-pool hand-offs. This is a soft target checked between
/// entries: a batch can overshoot it by one entry, i.e. at most one medium
/// file (up to `SMALL_FILE_THRESHOLD`) plus the framing flushed before it.
const MAX_BATCH_BYTES: usize = 512 * 1024;
/// Upper bound on the padding and closing `)` tokens after a file's contents.
const MAX_FILE_TRAILER_LEN: usize = 7 + 2 * TOK_PAR.len();

struct ActiveFile {
    fd: OwnedFd,
    remaining: u64,
    total_size: u64,
}

struct StreamWorker {
    walker: DirWalker,
    level: u32,
    active_file: Option<ActiveFile>,
    frame: Vec<u8>,
    chunks: VecDeque<io::Result<Bytes>>,
    done: bool,
}

impl StreamWorker {
    fn new(path: PathBuf) -> Self {
        #[cfg(target_os = "macos")]
        let use_case_hack = true;
        #[cfg(not(target_os = "macos"))]
        let use_case_hack = false;

        Self {
            walker: DirWalker::new(path, use_case_hack),
            level: 0,
            active_file: None,
            frame: Vec::new(),
            chunks: VecDeque::with_capacity(32),
            done: false,
        }
    }

    #[inline]
    fn put_padding(buf: &mut Vec<u8>, len: u64) {
        let pad = calc_padding(len);
        if pad > 0 {
            buf.extend_from_slice(&ZEROS[..pad]);
        }
    }

    #[inline]
    fn put_nix_slice(buf: &mut Vec<u8>, src: &[u8]) {
        buf.extend_from_slice(&(src.len() as u64).to_le_bytes());
        buf.extend_from_slice(src);
        Self::put_padding(buf, src.len() as u64);
    }

    #[inline]
    fn put_entry_header(buf: &mut Vec<u8>, name: &[u8]) {
        buf.extend_from_slice(TOK_ENTRY);
        Self::put_nix_slice(buf, name);
        buf.extend_from_slice(TOK_NODE);
    }

    #[inline]
    fn put_file_trailer(buf: &mut Vec<u8>, size: u64, level: u32) {
        Self::put_padding(buf, size);
        buf.extend_from_slice(TOK_PAR);
        if level > 0 {
            buf.extend_from_slice(TOK_PAR);
        }
    }

    fn flush_frame(
        frame: &mut Vec<u8>,
        chunks: &mut VecDeque<io::Result<Bytes>>,
        emitted: &mut usize,
    ) {
        if !frame.is_empty() {
            let chunk = std::mem::take(frame);
            *emitted += chunk.len();
            chunks.push_back(Ok(Bytes::from(chunk)));
        }
    }

    fn close_dirs_to(frame: &mut Vec<u8>, level: &mut u32, target_depth: usize) {
        while (*level as usize) > target_depth {
            frame.extend_from_slice(TOK_PAR);
            *level -= 1;
            if *level > 0 {
                frame.extend_from_slice(TOK_PAR);
            }
        }
    }

    fn fill_batch(&mut self) {
        self.encode_batch();
        if self.done {
            // Release directory descriptors as soon as the stream is terminal
            // (end or error) rather than when it is dropped.
            self.walker.close();
        }
    }

    fn encode_batch(&mut self) {
        let mut emitted = 0usize;

        while emitted < MAX_BATCH_BYTES {
            if self.frame.len() >= FRAME_FLUSH_THRESHOLD {
                Self::flush_frame(&mut self.frame, &mut self.chunks, &mut emitted);
                if emitted >= MAX_BATCH_BYTES {
                    break;
                }
            }

            match self.walker.next_event() {
                Ok(Some(ev)) => {
                    let depth = match &ev {
                        WalkEvent::Directory { depth, .. }
                        | WalkEvent::File { depth, .. }
                        | WalkEvent::Symlink { depth, .. } => *depth,
                    };
                    Self::close_dirs_to(&mut self.frame, &mut self.level, depth);
                    // Reserve a full chunk before the frame accumulates entries.
                    // Check capacity, not emptiness: a frame that starts with the
                    // trailer of a medium or large file is non-empty but freshly
                    // allocated. Framing before such a file is flushed at once,
                    // so it needs no reservation.
                    if self.frame.capacity() < FRAME_CHUNK_CAPACITY
                        && !matches!(&ev, WalkEvent::File { size, .. } if *size > INLINE_FILE_LIMIT as u64)
                    {
                        // Count bytes already in the frame, such as closing
                        // tokens, toward the target rather than on top of it.
                        self.frame
                            .reserve_exact(FRAME_CHUNK_CAPACITY - self.frame.len());
                    }
                    if self.level == 0 {
                        self.frame.extend_from_slice(TOK_ROOT);
                    }
                    match ev {
                        WalkEvent::Directory { name, .. } => {
                            if self.level > 0 {
                                Self::put_entry_header(&mut self.frame, name);
                            }
                            self.frame.extend_from_slice(TOK_DIR);
                            self.level += 1;
                        }
                        WalkEvent::Symlink { name, target, .. } => {
                            if self.level > 0 {
                                Self::put_entry_header(&mut self.frame, name);
                            }
                            self.frame.extend_from_slice(TOK_SYM);
                            Self::put_nix_slice(&mut self.frame, target);
                            self.frame.extend_from_slice(TOK_PAR);
                            if self.level > 0 {
                                self.frame.extend_from_slice(TOK_PAR);
                            }
                        }
                        WalkEvent::File {
                            name,
                            fd,
                            size,
                            executable,
                            ..
                        } => {
                            if self.level > 0 {
                                Self::put_entry_header(&mut self.frame, name);
                            }
                            self.frame.extend_from_slice(if executable {
                                TOK_FILE_E
                            } else {
                                TOK_FILE
                            });
                            self.frame.extend_from_slice(&size.to_le_bytes());

                            if size <= INLINE_FILE_LIMIT as u64 {
                                // Deep directory closes or a long name can leave too
                                // little room for the file in this chunk; emit the
                                // framing so far instead of growing past the
                                // reserved capacity.
                                if self.frame.len() + size as usize + MAX_FILE_TRAILER_LEN
                                    > FRAME_CHUNK_CAPACITY
                                {
                                    Self::flush_frame(
                                        &mut self.frame,
                                        &mut self.chunks,
                                        &mut emitted,
                                    );
                                    self.frame.reserve_exact(FRAME_CHUNK_CAPACITY);
                                }
                                if let Err(e) = read_whole_file_uninit(
                                    fd.as_fd(),
                                    &mut self.frame,
                                    size as usize,
                                ) {
                                    self.done = true;
                                    self.chunks.push_back(Err(e));
                                    return;
                                }
                                Self::put_file_trailer(&mut self.frame, size, self.level);
                            } else if size <= SMALL_FILE_THRESHOLD {
                                // Medium file: emit framing and read in sub-mmap chunks
                                // within this batch without yielding to the async runtime.
                                Self::flush_frame(&mut self.frame, &mut self.chunks, &mut emitted);
                                let mut rem = size as usize;
                                while rem > 0 {
                                    let want = rem.min(STREAM_FILE_CHUNK_SIZE);
                                    match read_chunk_uninit(fd.as_fd(), want) {
                                        Ok(buf) => {
                                            emitted += buf.len();
                                            rem -= want;
                                            self.chunks.push_back(Ok(Bytes::from(buf)));
                                        }
                                        Err(e) => {
                                            self.done = true;
                                            self.chunks.push_back(Err(e));
                                            return;
                                        }
                                    }
                                }
                                Self::put_file_trailer(&mut self.frame, size, self.level);
                            } else {
                                // Large file (>256 KiB): flush framing and hand the open
                                // descriptor back to `poll_next` so chunks stream one at
                                // a time without buffering megabytes simultaneously.
                                Self::flush_frame(&mut self.frame, &mut self.chunks, &mut emitted);
                                self.active_file = Some(ActiveFile {
                                    fd,
                                    remaining: size,
                                    total_size: size,
                                });
                                break;
                            }
                        }
                    }
                }
                Ok(None) => {
                    Self::close_dirs_to(&mut self.frame, &mut self.level, 0);
                    if !self.frame.is_empty() {
                        let chunk = std::mem::take(&mut self.frame);
                        self.chunks.push_back(Ok(Bytes::from(chunk)));
                    }
                    self.done = true;
                    break;
                }
                Err(e) => {
                    self.done = true;
                    self.chunks.push_back(Err(e));
                    break;
                }
            }
        }
    }
}

enum State {
    Idle(Option<Box<StreamWorker>>),
    Pending(JoinHandle<Box<StreamWorker>>),
}

/// A [`Stream`] of [`Bytes`] chunks containing NAR-encoded data for a path.
///
/// Walks the filesystem using dirfd-relative syscalls on a blocking worker and
/// encodes NAR wire framing directly into bounded chunks: small files are read
/// straight into the framing buffer with no intermediate allocation, and large
/// files are streamed in bounded chunks.
pub struct NarByteStream {
    state: State,
}

impl NarByteStream {
    /// Create a new `NarByteStream` for the given filesystem path.
    pub fn new(path: PathBuf) -> Self {
        Self {
            state: State::Idle(Some(Box::new(StreamWorker::new(path)))),
        }
    }
}

impl Stream for NarByteStream {
    type Item = io::Result<Bytes>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        loop {
            match &mut this.state {
                State::Idle(worker) => {
                    // `None` only after a cancelled batch dropped the worker.
                    let Some(w) = worker.as_mut() else {
                        return Poll::Ready(None);
                    };
                    if let Some(chunk) = w.chunks.pop_front() {
                        return Poll::Ready(Some(chunk));
                    }
                    if let Some(active) = w.active_file.as_mut() {
                        let want = active.remaining.min(STREAM_FILE_CHUNK_SIZE as u64) as usize;
                        match read_chunk_uninit(active.fd.as_fd(), want) {
                            Ok(buf) => {
                                active.remaining -= want as u64;
                                if active.remaining == 0 {
                                    let size = active.total_size;
                                    w.active_file = None;
                                    StreamWorker::put_file_trailer(&mut w.frame, size, w.level);
                                    // A root-level file has no siblings or parent directories,
                                    // so flush its trailer directly and skip a trailing pool hop.
                                    if w.level == 0 {
                                        let trailer = std::mem::take(&mut w.frame);
                                        w.chunks.push_back(Ok(Bytes::from(trailer)));
                                        w.done = true;
                                    }
                                }
                                return Poll::Ready(Some(Ok(Bytes::from(buf))));
                            }
                            Err(e) => {
                                w.active_file = None;
                                w.walker.close();
                                w.done = true;
                                return Poll::Ready(Some(Err(e)));
                            }
                        }
                    }
                    if w.done {
                        return Poll::Ready(None);
                    }
                    let mut w = worker.take().unwrap();
                    this.state = State::Pending(spawn_blocking(move || {
                        w.fill_batch();
                        w
                    }));
                }
                State::Pending(handle) => match ready!(Pin::new(handle).poll(cx)) {
                    Ok(w) => this.state = State::Idle(Some(w)),
                    Err(e) if e.is_panic() => std::panic::resume_unwind(e.into_panic()),
                    // The runtime shut down before the batch ran, dropping the
                    // worker with the task: report it and end the stream.
                    Err(e) => {
                        this.state = State::Idle(None);
                        return Poll::Ready(Some(Err(io::Error::from(e))));
                    }
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::test_data;
    use crate::archive::write_nar;
    use futures_util::StreamExt as _;

    async fn collect(path: PathBuf) -> Vec<u8> {
        let mut s = NarByteStream::new(path);
        let mut out = Vec::new();
        while let Some(chunk) = s.next().await {
            out.extend_from_slice(&chunk.unwrap());
        }
        out
    }

    /// The byte stream must produce the exact same bytes as the reference
    /// [`NarWriter`] for every test fixture.
    #[tokio::test]
    async fn byte_stream_matches_nar_writer() {
        let dir = tempfile::Builder::new()
            .prefix("nar_byte_stream")
            .tempdir()
            .unwrap();
        let path = dir.path().join("nar");
        // `NarByteStream::new` uses the platform default for case-hack
        // (enabled on macOS, disabled elsewhere); the on-disk fixture must be
        // created with the matching flag so the case-colliding `Deep`/`deep`
        // pair round-trips on case-insensitive filesystems.
        let case_hack = cfg!(target_os = "macos");
        test_data::create_dir_example(&path, case_hack).unwrap();

        let got = collect(path.clone()).await;
        let want = write_nar(test_data::dir_example().iter());
        assert_eq!(got, want.to_vec());
    }

    #[tokio::test]
    async fn byte_stream_single_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hello.txt");
        std::fs::write(&path, b"Hello world!").unwrap();

        let got = collect(path).await;
        let want = write_nar(test_data::text_file().iter());
        assert_eq!(got, want.to_vec());
    }

    /// Closing several directories right before a maximal inline file must
    /// not grow a chunk past its reserved capacity.
    #[tokio::test]
    async fn byte_stream_inline_file_after_deep_close_fits_chunk() {
        const DEPTH: usize = 8;
        fn nix_str(len: usize) -> usize {
            8 + len + calc_padding(len as u64)
        }
        fn file_len(name: &str, size: usize) -> usize {
            TOK_ENTRY.len()
                + nix_str(name.len())
                + TOK_NODE.len()
                + TOK_FILE.len()
                + 8
                + size
                + calc_padding(size as u64)
                + 2 * TOK_PAR.len()
        }
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("nar");
        let mut deep = root.clone();
        let mut framing = TOK_ROOT.len() + TOK_DIR.len();
        for i in 0..DEPTH {
            let name = format!("d{i}");
            framing += TOK_ENTRY.len() + nix_str(name.len()) + TOK_NODE.len() + TOK_DIR.len();
            deep.push(name);
        }
        std::fs::create_dir_all(&deep).unwrap();
        // Fill the frame to just below the flush threshold with two inline
        // files, then close all DEPTH directories before a maximal inline file.
        let filled = FRAME_FLUSH_THRESHOLD - 8;
        let first = 32 * 1024;
        let second = filled - framing - file_len("x1", first) - file_len("x2", 0);
        std::fs::write(deep.join("x1"), vec![1u8; first]).unwrap();
        std::fs::write(deep.join("x2"), vec![2u8; second]).unwrap();
        std::fs::write(root.join("z"), vec![3u8; INLINE_FILE_LIMIT]).unwrap();
        let unguarded = filled + DEPTH * 2 * TOK_PAR.len() + file_len("z", INLINE_FILE_LIMIT);
        assert!(
            unguarded > FRAME_CHUNK_CAPACITY,
            "fixture does not overflow"
        );

        let mut s = NarByteStream::new(root.clone());
        let mut got = Vec::new();
        while let Some(chunk) = s.next().await {
            let chunk = chunk.unwrap();
            assert!(
                chunk.len() <= FRAME_CHUNK_CAPACITY,
                "chunk of {} bytes",
                chunk.len()
            );
            got.extend_from_slice(&chunk);
        }
        let want = std::process::Command::new("nix-store")
            .arg("--dump")
            .arg(&root)
            .output()
            .expect("nix-store --dump failed");
        assert!(want.status.success());
        assert_eq!(got, want.stdout);
    }

    /// Once the stream reports an error its walker must not keep directory
    /// descriptors open until the stream is dropped.
    #[test]
    fn walker_error_releases_dir_fds() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a/b/c");
        std::fs::create_dir_all(&nested).unwrap();
        crate::archive::dumper::unittests::mkfifo(&nested.join("pipe"));

        let mut w = StreamWorker::new(dir.path().to_path_buf());
        while !w.done {
            w.fill_batch();
        }
        let err = w.chunks.pop_back().unwrap().expect_err("FIFO must fail");
        assert!(err.to_string().contains("unsupported file type"), "{err}");
        assert_eq!(w.walker.open_dir_count(), 0);
    }

    /// A batch cancelled by runtime shutdown is an error, not a panic, and
    /// ends the stream.
    #[test]
    fn cancelled_batch_is_an_error() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let handle = rt.handle().clone();
        drop(rt);
        // `spawn_blocking` on a shut-down runtime cancels the task at once.
        let _guard = handle.enter();
        let dir = tempfile::tempdir().unwrap();
        let mut s = NarByteStream::new(dir.path().to_path_buf());
        let mut cx = Context::from_waker(std::task::Waker::noop());
        match Pin::new(&mut s).poll_next(&mut cx) {
            Poll::Ready(Some(Err(e))) => assert!(e.to_string().contains("cancelled"), "{e}"),
            other => panic!("expected cancellation error, got {other:?}"),
        }
        assert!(matches!(
            Pin::new(&mut s).poll_next(&mut cx),
            Poll::Ready(None)
        ));
    }

    /// Exercise the chunked-read path for large payloads.
    #[tokio::test]
    async fn byte_stream_large_file_matches_nix_store_dump() {
        // Larger than INLINE_FILE_LIMIT and FILE_CHUNK_SIZE, and not a multiple
        // of 8 so padding and the final short slice are both covered.
        const LEN: usize = 300 * 1024 + 5;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big");
        let data: Vec<u8> = (0..LEN).map(|i| (i % 251) as u8).collect();
        std::fs::write(&path, &data).unwrap();

        let got = collect(path.clone()).await;

        let want = std::process::Command::new("nix-store")
            .arg("--dump")
            .arg(&path)
            .output()
            .expect("nix-store --dump failed");
        assert!(want.status.success());
        assert_eq!(got, want.stdout);
    }
}
