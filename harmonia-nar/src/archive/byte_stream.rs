use std::io;
use std::path::PathBuf;
use std::pin::Pin;
use std::task::{Context, Poll, ready};

use bytes::{Bytes, BytesMut};
use futures::{SinkExt, Stream, StreamExt};
use tokio::io::AsyncWrite;
use tokio::sync::mpsc;
use tokio_util::sync::PollSender;

use super::dumper::DumpOptions;
use super::writer::NarWriter;

/// Default chunk size for yielded Bytes (64 KiB).
const DEFAULT_CHUNK_SIZE: usize = 64 * 1024;

/// Number of Bytes chunks to buffer in the channel.
/// Provides pipelining: the NAR encoder can work ahead while the HTTP layer
/// is sending previous chunks, without unbounded memory growth.
const CHANNEL_CAPACITY: usize = 4;

/// An [`AsyncWrite`] that collects bytes into [`Bytes`] chunks and sends them
/// through a bounded mpsc channel via [`PollSender`].
///
/// When the internal buffer reaches `chunk_size`, it is frozen into a
/// [`Bytes`] and sent through the channel. If the channel is full,
/// `poll_write` returns [`Poll::Pending`] until the consumer drains a slot,
/// providing natural back-pressure.
struct ChannelWriter {
    sender: PollSender<Bytes>,
    buffer: BytesMut,
    chunk_size: usize,
}

impl ChannelWriter {
    fn new(sender: PollSender<Bytes>, chunk_size: usize) -> Self {
        Self {
            sender,
            buffer: BytesMut::with_capacity(chunk_size),
            chunk_size,
        }
    }

    /// Try to send the current buffer contents as a chunk.
    /// Returns `Poll::Pending` if the channel is full (back-pressure).
    fn poll_emit_chunk(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.buffer.is_empty() {
            return Poll::Ready(Ok(()));
        }

        // Reserve a slot in the channel before sending.
        ready!(self.sender.poll_reserve(cx))
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "channel closed"))?;

        let chunk = std::mem::replace(&mut self.buffer, BytesMut::with_capacity(self.chunk_size));
        self.sender
            .send_item(chunk.freeze())
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "channel closed"))?;

        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for ChannelWriter {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        // If the buffer is already full, flush it first (may return Pending).
        if self.buffer.len() >= self.chunk_size {
            ready!(self.poll_emit_chunk(cx))?;
        }

        let n = buf.len().min(self.chunk_size - self.buffer.len());
        self.buffer.extend_from_slice(&buf[..n]);
        Poll::Ready(Ok(n))
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        ready!(self.poll_emit_chunk(cx))?;
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        ready!(self.poll_emit_chunk(cx))?;
        Poll::Ready(Ok(()))
    }
}

/// A [`Stream`] of [`Bytes`] chunks containing NAR-encoded data.
///
/// Spawns a background task that walks the filesystem and encodes the NAR
/// wire format, sending `Bytes` chunks through a bounded mpsc channel. This
/// provides natural pipelining: the encoder runs concurrently with the HTTP
/// send path, while back-pressure from the channel prevents unbounded memory
/// growth for large store paths.
pub struct NarByteStream {
    rx: mpsc::Receiver<Bytes>,
}

impl NarByteStream {
    /// Create a new `NarByteStream` for the given filesystem path.
    pub fn new(path: PathBuf) -> Self {
        Self::with_chunk_size(path, DEFAULT_CHUNK_SIZE)
    }

    /// Create a new `NarByteStream` with a custom output chunk size.
    pub fn with_chunk_size(path: PathBuf, chunk_size: usize) -> Self {
        let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);
        let sender = PollSender::new(tx);

        tokio::task::spawn(async move {
            let writer = ChannelWriter::new(sender, chunk_size);
            let mut nar_writer = NarWriter::new(writer);
            let events = DumpOptions::new().dump(path);

            futures::pin_mut!(events);
            while let Some(event_result) = events.next().await {
                match event_result {
                    Ok(event) => {
                        if let Err(e) = nar_writer.send(event).await {
                            tracing::error!("Error writing NAR event: {}", e);
                            return;
                        }
                    }
                    Err(e) => {
                        tracing::error!("Error reading path for NAR dump: {}", e);
                        return;
                    }
                }
            }

            if let Err(e) = nar_writer.close().await {
                tracing::error!("Error closing NAR writer: {}", e);
            }
        });

        Self { rx }
    }
}

impl Stream for NarByteStream {
    type Item = io::Result<Bytes>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.rx.poll_recv(cx).map(|opt| opt.map(Ok))
    }
}
