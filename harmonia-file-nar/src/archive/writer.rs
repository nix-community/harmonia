use std::{
    io,
    pin::Pin,
    task::{Context, Poll, ready},
};

use bytes::BufMut;
use futures_sink::Sink;
use pin_project_lite::pin_project;
use tokio::io::{AsyncBufRead, AsyncWrite};

use crate::wire::calc_padding;
use harmonia_utils_io::{DEFAULT_BUF_SIZE, DEFAULT_RESERVED_BUF_SIZE};

use super::{
    NarEvent,
    read_nar::{TOK_DIR, TOK_ENTRY, TOK_FILE, TOK_FILE_E, TOK_NODE, TOK_PAR, TOK_ROOT, TOK_SYM},
};

#[derive(Clone, Copy)]
enum State {
    Ready,
    /// Copy `reader` into `buffer` until EOF (falling back to streaming if it exceeds `cutoff`).
    BufferFile {
        pad: u8,
        reserve: usize,
    },
    /// Stream `reader` directly to `writer` until EOF.
    StreamFile {
        pad: u8,
    },
}

pin_project! {
    pub struct NarWriter<R, W> {
        state: State,
        flushing: bool,
        buffer: Vec<u8>,
        write_pos: usize,
        cutoff: usize,
        #[pin]
        reader: Option<R>,
        #[pin]
        writer: W,
        level: usize,
    }
}

impl<R, W> NarWriter<R, W>
where
    W: AsyncWrite,
    R: AsyncBufRead,
{
    pub fn new(writer: W) -> Self {
        Self {
            state: State::Ready,
            flushing: false,
            buffer: Vec::new(),
            write_pos: 0,
            cutoff: DEFAULT_BUF_SIZE,
            reader: None,
            level: 0,
            writer,
        }
    }

    pub fn poll_flush_buf(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), io::Error>> {
        ready!(self.as_mut().poll_ready(cx))?;
        let this = self.project();
        poll_drain_buf(this.writer, this.buffer, this.write_pos, cx)
    }
}

fn put_nix_slice(buf: &mut Vec<u8>, src: &[u8]) {
    buf.put_u64_le(src.len() as u64);
    buf.put_slice(src);
    buf.put_bytes(0, calc_padding(src.len() as u64));
}

fn put_entry_header(buf: &mut Vec<u8>, level: usize, name: &[u8]) {
    if level > 0 {
        buf.put_slice(TOK_ENTRY);
        put_nix_slice(buf, name);
        buf.put_slice(TOK_NODE);
    }
}

fn finish_node(buf: &mut Vec<u8>, level: usize) {
    buf.put_slice(TOK_PAR);
    if level > 0 {
        buf.put_slice(TOK_PAR);
    }
}

fn poll_write_nonzero<W: AsyncWrite>(
    writer: Pin<&mut W>,
    cx: &mut Context<'_>,
    buf: &[u8],
) -> Poll<io::Result<usize>> {
    let cnt = ready!(writer.poll_write(cx, buf))?;
    if cnt == 0 {
        return Poll::Ready(Err(io::Error::new(
            io::ErrorKind::WriteZero,
            "wrote zero bytes",
        )));
    }
    Poll::Ready(Ok(cnt))
}

fn poll_drain_buf<W: AsyncWrite>(
    mut writer: Pin<&mut W>,
    buffer: &mut Vec<u8>,
    write_pos: &mut usize,
    cx: &mut Context<'_>,
) -> Poll<io::Result<()>> {
    while *write_pos < buffer.len() {
        let cnt = ready!(poll_write_nonzero(
            writer.as_mut(),
            cx,
            &buffer[*write_pos..]
        ))?;
        *write_pos += cnt;
    }
    buffer.clear();
    *write_pos = 0;
    Poll::Ready(Ok(()))
}

impl<R, W> Sink<NarEvent<R>> for NarWriter<R, W>
where
    W: AsyncWrite,
    R: AsyncBufRead,
{
    type Error = io::Error;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let mut this = self.project();
        loop {
            if *this.flushing {
                ready!(poll_drain_buf(
                    this.writer.as_mut(),
                    this.buffer,
                    this.write_pos,
                    cx,
                ))?;
                *this.flushing = false;
            }
            match *this.state {
                State::Ready => {
                    if this.buffer.len() > *this.cutoff {
                        *this.flushing = true;
                        continue;
                    }
                    return Poll::Ready(Ok(()));
                }
                State::BufferFile {
                    pad,
                    ref mut reserve,
                } => {
                    if *reserve > 0 {
                        this.buffer.reserve(*reserve);
                        *reserve = 0;
                    }
                    let mut reader = this
                        .reader
                        .as_mut()
                        .as_pin_mut()
                        .expect("reader present in BufferFile state");
                    loop {
                        let buf = ready!(reader.as_mut().poll_fill_buf(cx))?;
                        if buf.is_empty() {
                            this.reader.set(None);
                            if pad > 0 {
                                this.buffer.put_bytes(0, pad as usize);
                            }
                            finish_node(this.buffer, *this.level);
                            *this.state = State::Ready;
                            break;
                        }
                        // If the reader yields more bytes than fit in the buffer
                        // cutoff, flush and switch to direct streaming without
                        // growing the buffer unboundedly.
                        if this.buffer.len().saturating_add(buf.len()) > *this.cutoff {
                            *this.flushing = true;
                            *this.state = State::StreamFile { pad };
                            break;
                        }
                        let len = buf.len();
                        this.buffer.put_slice(buf);
                        reader.as_mut().consume(len);
                    }
                }
                State::StreamFile { pad } => {
                    let mut reader = this
                        .reader
                        .as_mut()
                        .as_pin_mut()
                        .expect("reader present in StreamFile state");
                    loop {
                        let buf = ready!(reader.as_mut().poll_fill_buf(cx))?;
                        if buf.is_empty() {
                            break;
                        }
                        let amt = ready!(poll_write_nonzero(this.writer.as_mut(), cx, buf))?;
                        reader.as_mut().consume(amt);
                    }
                    this.reader.set(None);
                    if pad > 0 {
                        this.buffer.put_bytes(0, pad as usize);
                    }
                    finish_node(this.buffer, *this.level);
                    *this.state = State::Ready;
                }
            }
        }
    }

    fn start_send(self: Pin<&mut Self>, item: NarEvent<R>) -> Result<(), Self::Error> {
        let mut this = self.project();
        if *this.level == 0 {
            this.buffer.put_slice(TOK_ROOT);
        }
        match item {
            NarEvent::File {
                name,
                executable,
                size,
                reader,
            } => {
                put_entry_header(this.buffer, *this.level, &name);
                if executable {
                    this.buffer.put_slice(TOK_FILE_E);
                } else {
                    this.buffer.put_slice(TOK_FILE);
                }
                this.buffer.put_u64_le(size);
                let pad = calc_padding(size) as u8;
                let closing_len = if *this.level == 0 {
                    TOK_PAR.len()
                } else {
                    TOK_PAR.len() * 2
                };
                let inline_limit = (*this.cutoff).min(DEFAULT_RESERVED_BUF_SIZE) as u64;
                let total_file_len = size
                    .saturating_add(pad as u64)
                    .saturating_add(closing_len as u64);
                if total_file_len <= inline_limit {
                    let reserve = total_file_len as usize;
                    if this.buffer.len().saturating_add(reserve) > *this.cutoff {
                        *this.flushing = true;
                    }
                    *this.state = State::BufferFile { pad, reserve };
                } else {
                    *this.flushing = true;
                    *this.state = State::StreamFile { pad };
                }
                this.reader.set(Some(reader));
            }
            NarEvent::Symlink { name, target } => {
                put_entry_header(this.buffer, *this.level, &name);
                this.buffer.put_slice(TOK_SYM);
                put_nix_slice(this.buffer, &target);
                finish_node(this.buffer, *this.level);
            }
            NarEvent::StartDirectory { name } => {
                put_entry_header(this.buffer, *this.level, &name);
                this.buffer.put_slice(TOK_DIR);
                *this.level += 1;
            }
            NarEvent::EndDirectory => {
                *this.level -= 1;
                finish_node(this.buffer, *this.level);
            }
        }
        Ok(())
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        ready!(self.as_mut().poll_flush_buf(cx))?;
        self.project().writer.poll_flush(cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        ready!(self.as_mut().poll_flush_buf(cx))?;
        self.project().writer.poll_shutdown(cx)
    }
}
#[cfg(any(test, feature = "test"))]
pub fn write_nar<'e, E>(events: E) -> bytes::Bytes
where
    E: IntoIterator<Item = &'e super::test_data::TestNarEvent>,
{
    use futures_util::{FutureExt as _, SinkExt as _, StreamExt as _, stream::iter};
    use std::io::Cursor;

    let mut buf = Vec::new();
    let mut writer = NarWriter::new(Cursor::new(&mut buf));
    let mut stream = iter(events).map(Clone::clone).map(Ok);
    writer
        .send_all(&mut stream)
        .now_or_never()
        .expect("BUG: NarWriter blocks")
        .expect("BUG: NarWriter returned error");
    writer
        .close()
        .now_or_never()
        .expect("BUG: NarWriter close blocks")
        .expect("BUG: NarWriter close errors");
    buf.into()
}

#[cfg(test)]
mod unittests {
    use std::io::{self, Cursor};
    use std::pin::Pin;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::task::{Context, Poll, ready};

    use bytes::Bytes;
    use futures_util::stream::iter;
    use futures_util::{SinkExt as _, StreamExt as _};
    use rstest::rstest;
    use tempfile::tempdir;
    use tokio::fs::File;
    use tokio::io::{AsyncBufRead, AsyncRead, AsyncWrite, ReadBuf};

    use crate::archive::{NarEvent, read_nar, test_data};
    use harmonia_utils_io::BytesReader;

    use super::NarWriter;

    #[tokio::test]
    #[rstest]
    #[case::text_file(test_data::text_file(), test_data::text_file())]
    #[case::exec_file(test_data::exec_file(), test_data::exec_file())]
    #[case::empty_file(test_data::empty_file(), test_data::empty_file())]
    #[case::empty_file_in_dir(test_data::empty_file_in_dir(), test_data::empty_file_in_dir())]
    #[case::empty_dir(test_data::empty_dir(), test_data::empty_dir())]
    #[case::empty_dir_in_dir(test_data::empty_dir_in_dir(), test_data::empty_dir_in_dir())]
    #[case::symlink(test_data::symlink(), test_data::symlink())]
    #[case::dir_example(test_data::dir_example(), test_data::dir_example())]
    async fn write_test_data(
        #[case] events: test_data::TestNarEvents,
        #[case] expected: test_data::TestNarEvents,
    ) {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test-text.nar");

        let io = File::create(&path).await.unwrap();
        let encoder = NarWriter::new(io);
        let stream = iter(events).map(Ok);
        stream.forward(encoder).await.unwrap();

        let io = File::open(path).await.unwrap();
        let s = read_nar(BytesReader::new(io)).await.unwrap();
        assert_eq!(s, expected);
    }

    #[tokio::test]
    async fn coalesces_small_files_and_streams_large_files() {
        struct RecordingWriter {
            writes: Arc<AtomicUsize>,
            out: Arc<Mutex<Vec<u8>>>,
        }
        impl AsyncWrite for RecordingWriter {
            fn poll_write(
                self: Pin<&mut Self>,
                _cx: &mut Context<'_>,
                buf: &[u8],
            ) -> Poll<io::Result<usize>> {
                self.writes.fetch_add(1, Ordering::Relaxed);
                self.out.lock().unwrap().extend_from_slice(buf);
                Poll::Ready(Ok(buf.len()))
            }
            fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
                Poll::Ready(Ok(()))
            }
            fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
                Poll::Ready(Ok(()))
            }
        }

        // Mix small files across multiple 32 KiB cutoff batches (exercising both
        // BufferFile and flush-then-buffer), empty files, and an unaligned large
        // file exercising StreamFile with non-zero trailing padding.
        let mut expected = vec![test_data::TestNarEvent::StartDirectory { name: Bytes::new() }];
        for i in 0..80u32 {
            let len = if i % 10 == 0 {
                0
            } else {
                500 + (i % 13) as usize
            };
            expected.push(test_data::TestNarEvent::File {
                name: Bytes::from(format!("f_{i:02}")),
                executable: i % 2 == 0,
                size: len as u64,
                reader: Cursor::new(Bytes::from(vec![b'a' + (i % 26) as u8; len])),
            });
        }
        let big = Bytes::from(vec![b'Z'; 20_003]);
        expected.push(test_data::TestNarEvent::File {
            name: Bytes::from_static(b"z_big"),
            executable: false,
            size: big.len() as u64,
            reader: Cursor::new(big),
        });
        expected.push(test_data::TestNarEvent::EndDirectory);

        let writes = Arc::new(AtomicUsize::new(0));
        let out = Arc::new(Mutex::new(Vec::new()));
        let encoder = NarWriter::new(RecordingWriter {
            writes: writes.clone(),
            out: out.clone(),
        });
        let stream = iter(expected.clone()).map(Ok);
        stream.forward(encoder).await.unwrap();

        // 81 files (~68 KiB total) coalesce into ~5 writes instead of >160.
        assert!(writes.load(Ordering::Relaxed) < 12);
        let nar = out.lock().unwrap().clone();
        let parsed = read_nar(BytesReader::new(Cursor::new(nar))).await.unwrap();
        assert_eq!(parsed, expected);
    }

    #[tokio::test]
    async fn handles_partial_writes_pending_and_buffer_overflow_fallback() {
        struct TrickleReader {
            data: Vec<u8>,
            pos: usize,
            chunk: usize,
            pending_next: bool,
        }
        impl AsyncRead for TrickleReader {
            fn poll_read(
                mut self: Pin<&mut Self>,
                cx: &mut Context<'_>,
                buf: &mut ReadBuf<'_>,
            ) -> Poll<io::Result<()>> {
                let avail = ready!(self.as_mut().poll_fill_buf(cx))?;
                let n = avail.len().min(buf.remaining());
                buf.put_slice(&avail[..n]);
                self.consume(n);
                Poll::Ready(Ok(()))
            }
        }
        impl AsyncBufRead for TrickleReader {
            fn poll_fill_buf(
                mut self: Pin<&mut Self>,
                cx: &mut Context<'_>,
            ) -> Poll<io::Result<&[u8]>> {
                if self.pending_next {
                    self.pending_next = false;
                    cx.waker().wake_by_ref();
                    return Poll::Pending;
                }
                self.pending_next = true;
                let start = self.pos;
                let end = (start + self.chunk).min(self.data.len());
                Poll::Ready(Ok(&self.get_mut().data[start..end]))
            }
            fn consume(mut self: Pin<&mut Self>, amt: usize) {
                self.pos += amt;
            }
        }

        struct TrickleWriter {
            out: Arc<Mutex<Vec<u8>>>,
            max_write: usize,
            pending_next: bool,
        }
        impl AsyncWrite for TrickleWriter {
            fn poll_write(
                mut self: Pin<&mut Self>,
                cx: &mut Context<'_>,
                buf: &[u8],
            ) -> Poll<io::Result<usize>> {
                if self.pending_next {
                    self.pending_next = false;
                    cx.waker().wake_by_ref();
                    return Poll::Pending;
                }
                self.pending_next = true;
                let n = buf.len().min(self.max_write);
                self.out.lock().unwrap().extend_from_slice(&buf[..n]);
                Poll::Ready(Ok(n))
            }
            fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
                Poll::Ready(Ok(()))
            }
            fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
                Poll::Ready(Ok(()))
            }
        }

        let small = vec![b's'; 1003];
        let big = vec![b'b'; 10_005];
        let out = Arc::new(Mutex::new(Vec::new()));
        let mut writer = NarWriter::new(TrickleWriter {
            out: out.clone(),
            max_write: 137,
            pending_next: true,
        });
        writer
            .feed(NarEvent::StartDirectory { name: Bytes::new() })
            .await
            .unwrap();
        writer
            .feed(NarEvent::File {
                name: Bytes::from_static(b"a_small"),
                executable: true,
                size: small.len() as u64,
                reader: TrickleReader {
                    data: small.clone(),
                    pos: 0,
                    chunk: 113,
                    pending_next: true,
                },
            })
            .await
            .unwrap();
        writer
            .feed(NarEvent::File {
                name: Bytes::from_static(b"b_big"),
                executable: false,
                size: big.len() as u64,
                reader: TrickleReader {
                    data: big.clone(),
                    pos: 0,
                    chunk: 311,
                    pending_next: true,
                },
            })
            .await
            .unwrap();
        writer.feed(NarEvent::EndDirectory).await.unwrap();
        writer.close().await.unwrap();

        let expected = vec![
            test_data::TestNarEvent::StartDirectory { name: Bytes::new() },
            test_data::TestNarEvent::File {
                name: Bytes::from_static(b"a_small"),
                executable: true,
                size: small.len() as u64,
                reader: Cursor::new(Bytes::from(small)),
            },
            test_data::TestNarEvent::File {
                name: Bytes::from_static(b"b_big"),
                executable: false,
                size: big.len() as u64,
                reader: Cursor::new(Bytes::from(big)),
            },
            test_data::TestNarEvent::EndDirectory,
        ];
        let nar = out.lock().unwrap().clone();
        let parsed = read_nar(BytesReader::new(Cursor::new(nar))).await.unwrap();
        assert_eq!(parsed, expected);
    }

    #[tokio::test]
    async fn stream_file_write_zero_returns_error() {
        struct ZeroOnSecondWrite {
            calls: usize,
        }
        impl AsyncWrite for ZeroOnSecondWrite {
            fn poll_write(
                mut self: Pin<&mut Self>,
                _cx: &mut Context<'_>,
                buf: &[u8],
            ) -> Poll<io::Result<usize>> {
                self.calls += 1;
                if self.calls == 1 {
                    Poll::Ready(Ok(buf.len()))
                } else {
                    Poll::Ready(Ok(0))
                }
            }
            fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
                Poll::Ready(Ok(()))
            }
            fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
                Poll::Ready(Ok(()))
            }
        }

        let big = Bytes::from(vec![b'x'; 20_000]);
        let mut writer = NarWriter::new(ZeroOnSecondWrite { calls: 0 });
        writer
            .feed(NarEvent::File {
                name: Bytes::new(),
                executable: false,
                size: big.len() as u64,
                reader: Cursor::new(big),
            })
            .await
            .unwrap();
        let err = writer.close().await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::WriteZero);
    }
}
