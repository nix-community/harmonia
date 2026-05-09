use std::io;
use std::path::PathBuf;
use std::pin::Pin;
use std::task::{Context, Poll, ready};

use bytes::{BufMut, Bytes, BytesMut};
use futures_core::Stream;

use super::NarEvent;
use super::dumper::{DumpOptions, NarDumper};
use super::read_nar::{
    TOK_DIR, TOK_ENTRY, TOK_FILE, TOK_FILE_E, TOK_NODE, TOK_PAR, TOK_ROOT, TOK_SYM,
};
use crate::wire::calc_padding;

/// Flush accumulated framing once it exceeds this many bytes so a long run of
/// tiny entries (symlinks, empty dirs) does not buffer unbounded amounts of
/// metadata before yielding to the consumer.
const FRAME_FLUSH_THRESHOLD: usize = 32 * 1024;

/// Max size of a single file-content chunk yielded downstream.
///
/// Large mmap'd files are sliced into pieces of this size so the HTTP layer
/// can interleave socket writes with further NAR encoding work and so a slow
/// client does not pin a multi-GiB `Bytes` in actix's write buffer at once.
/// Slicing a `Bytes` is just a refcount bump — no copy.
const FILE_CHUNK_SIZE: usize = 256 * 1024;

enum Phase {
    /// Pull the next [`NarEvent`] from the dumper and encode framing.
    Event,
    /// Yield file content (possibly in multiple slices), then append the
    /// trailing padding/`)` tokens to `frame` and go back to [`Phase::Event`].
    Emit {
        data: Bytes,
        size: u64,
    },
    Done,
}

/// A [`Stream`] of [`Bytes`] chunks containing NAR-encoded data for a path.
///
/// Drives a [`NarDumper`] directly and emits the NAR wire format without
/// intermediate copies: framing tokens are accumulated in a small reusable
/// buffer, and file payloads are forwarded as the `Bytes` already loaded by
/// the dumper (heap buffer for small files, mmap-backed for large ones). The
/// only per-byte copy on the serving path is the one the HTTP layer performs
/// into its socket write buffer.
pub struct NarByteStream {
    dumper: NarDumper,
    /// Scratch buffer for NAR structure tokens between file payloads.
    frame: BytesMut,
    /// Directory nesting depth, mirroring [`NarWriter`]'s `level` so the
    /// emitted token sequence is byte-identical.
    level: u32,
    phase: Phase,
}

impl NarByteStream {
    /// Create a new `NarByteStream` for the given filesystem path.
    pub fn new(path: PathBuf) -> Self {
        Self {
            dumper: DumpOptions::new().dump(path),
            frame: BytesMut::with_capacity(FRAME_FLUSH_THRESHOLD),
            level: 0,
            phase: Phase::Event,
        }
    }

    /// Append the entry-prefix tokens that precede every non-root node.
    fn put_entry_header(frame: &mut BytesMut, name: &[u8]) {
        frame.put_slice(TOK_ENTRY);
        put_nix_slice(frame, name);
        frame.put_slice(TOK_NODE);
    }
}

fn put_nix_slice(buf: &mut BytesMut, src: &[u8]) {
    buf.put_u64_le(src.len() as u64);
    buf.put_slice(src);
    buf.put_bytes(0, calc_padding(src.len() as u64));
}

impl Stream for NarByteStream {
    type Item = io::Result<Bytes>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        loop {
            match &mut this.phase {
                Phase::Event => {
                    if this.frame.len() >= FRAME_FLUSH_THRESHOLD {
                        return Poll::Ready(Some(Ok(this.frame.split().freeze())));
                    }
                    match ready!(Pin::new(&mut this.dumper).poll_next(cx)) {
                        Some(Ok(event)) => {
                            if this.level == 0 {
                                this.frame.put_slice(TOK_ROOT);
                            }
                            match event {
                                NarEvent::StartDirectory { name } => {
                                    if this.level > 0 {
                                        Self::put_entry_header(&mut this.frame, &name);
                                    }
                                    this.frame.put_slice(TOK_DIR);
                                    this.level += 1;
                                }
                                NarEvent::EndDirectory => {
                                    this.frame.put_slice(TOK_PAR);
                                    this.level -= 1;
                                    if this.level > 0 {
                                        this.frame.put_slice(TOK_PAR);
                                    }
                                }
                                NarEvent::Symlink { name, target } => {
                                    if this.level > 0 {
                                        Self::put_entry_header(&mut this.frame, &name);
                                    }
                                    this.frame.put_slice(TOK_SYM);
                                    put_nix_slice(&mut this.frame, &target);
                                    this.frame.put_slice(TOK_PAR);
                                    if this.level > 0 {
                                        this.frame.put_slice(TOK_PAR);
                                    }
                                }
                                NarEvent::File {
                                    name,
                                    executable,
                                    size,
                                    reader,
                                } => {
                                    if this.level > 0 {
                                        Self::put_entry_header(&mut this.frame, &name);
                                    }
                                    this.frame.put_slice(if executable {
                                        TOK_FILE_E
                                    } else {
                                        TOK_FILE
                                    });
                                    this.frame.put_u64_le(size);
                                    this.phase = Phase::Emit {
                                        data: reader.into_bytes(),
                                        size,
                                    };
                                    // Flush framing now so file bytes follow
                                    // immediately without being copied into
                                    // the frame buffer.
                                    if !this.frame.is_empty() {
                                        return Poll::Ready(Some(Ok(this.frame.split().freeze())));
                                    }
                                }
                            }
                        }
                        Some(Err(e)) => return Poll::Ready(Some(Err(e))),
                        None => {
                            this.phase = Phase::Done;
                            if !this.frame.is_empty() {
                                return Poll::Ready(Some(Ok(this.frame.split().freeze())));
                            }
                        }
                    }
                }
                Phase::Emit { data, size } => {
                    if !data.is_empty() {
                        let n = data.len().min(FILE_CHUNK_SIZE);
                        let chunk = data.split_to(n);
                        return Poll::Ready(Some(Ok(chunk)));
                    }
                    // File fully emitted; append trailer and resume framing.
                    this.frame.put_bytes(0, calc_padding(*size));
                    this.frame.put_slice(TOK_PAR);
                    if this.level > 0 {
                        this.frame.put_slice(TOK_PAR);
                    }
                    this.phase = Phase::Event;
                }
                Phase::Done => return Poll::Ready(None),
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

    /// The zero-copy byte stream must produce the exact same bytes as the
    /// reference [`NarWriter`] for every test fixture.
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

    /// Exercise the mmap-backed `Bytes::from_owner` path and the
    /// `FILE_CHUNK_SIZE` slicing of large payloads.
    #[tokio::test]
    async fn byte_stream_large_file_matches_nix_store_dump() {
        // Larger than SMALL_FILE_THRESHOLD and not a multiple of
        // FILE_CHUNK_SIZE / 8, so padding and the final short slice are both
        // covered.
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
