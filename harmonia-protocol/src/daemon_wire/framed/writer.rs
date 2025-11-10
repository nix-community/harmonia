use std::cmp::min;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, ready};

use bytes::{Buf as _, BufMut, BytesMut};
use pin_project_lite::pin_project;
use tokio::io::AsyncWrite;
use tracing::{Span, debug, trace, trace_span};

use crate::io::{DEFAULT_BUF_SIZE, RESERVED_BUF_SIZE};

pin_project! {
    #[derive(Debug)]
    pub struct FramedWriter<W> {
        #[pin]
        inner: W,
        flushing: Option<Span>,
        shutdown: bool,
        buf: BytesMut,
    }
}

impl<W> FramedWriter<W>
where
    W: AsyncWrite,
{
    pub fn new(writer: W) -> Self {
        let mut buf = BytesMut::with_capacity(DEFAULT_BUF_SIZE);
        buf.put_u64_le(0);
        FramedWriter {
            inner: writer,
            flushing: None,
            shutdown: false,
            buf,
        }
    }
    fn poll_flush_buf(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        let mut this = self.project();
        if this.flushing.is_none() && this.buf.len() > 8 {
            let len = this.buf.len() - 8;
            debug!(
                len,
                cap = this.buf.capacity(),
                "FramedWriter:poll_flush_buf: write frame"
            );
            BufMut::put_u64_le(&mut &mut this.buf[..8], len as u64);
            *this.flushing = Some(trace_span!(
                "FramedWriter::poll_flush_buf",
                len,
                cap = this.buf.capacity()
            ));
        }
        if let Some(span) = this.flushing.as_ref() {
            let _ = span.enter();
            while !this.buf.is_empty() {
                trace!(
                    remaning = this.buf.len(),
                    cap = this.buf.capacity(),
                    "FramedWriter:poll_flush_buf: write {}",
                    this.buf.len()
                );
                let n = ready!(this.inner.as_mut().poll_write(cx, &this.buf[..]))?;
                if n == 0 {
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "failed to write the buffer",
                    )));
                }
                this.buf.advance(n);
            }
            this.buf.reserve(RESERVED_BUF_SIZE);
            this.buf.put_u64_le(0);
            *this.flushing = None;
        } else {
            this.buf.reserve(RESERVED_BUF_SIZE);
        }
        trace!(
            len = this.buf.len(),
            cap = this.buf.capacity(),
            "FramedWriter:poll_flush_buf: done"
        );
        Poll::Ready(Ok(()))
    }

    pub fn remaining_mut(&self) -> usize {
        self.buf.capacity() - self.buf.len()
    }
}

impl<W> AsyncWrite for FramedWriter<W>
where
    W: AsyncWrite,
{
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        trace!(
            len = self.buf.len(),
            cap = self.buf.capacity(),
            "FramedWriter:poll_write: check buf"
        );
        if self.remaining_mut() == 0 || self.flushing.is_some() {
            ready!(self.as_mut().poll_flush_buf(cx))?;
        }
        let write = min(buf.len(), self.remaining_mut());
        trace!(
            len = self.buf.len(),
            cap = self.buf.capacity(),
            "FramedWriter:poll_write: write slice"
        );
        self.as_mut().project().buf.put_slice(&buf[..write]);
        trace!(
            len = self.buf.len(),
            cap = self.buf.capacity(),
            "FramedWriter:poll_write: done"
        );
        Poll::Ready(Ok(write))
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        trace!(
            len = self.buf.len(),
            cap = self.buf.capacity(),
            "FramedWriter:poll_flush: flush buf"
        );
        ready!(self.as_mut().poll_flush_buf(cx))?;
        trace!(
            len = self.buf.len(),
            cap = self.buf.capacity(),
            "FramedWriter:poll_flush: flush writer"
        );
        self.project().inner.poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), io::Error>> {
        trace!(
            len = self.buf.len(),
            cap = self.buf.capacity(),
            "FramedWriter:poll_shutdown: flush"
        );
        ready!(self.as_mut().poll_flush_buf(cx))?;
        let mut this = self.as_mut().project();
        if !*this.shutdown {
            while !this.buf.is_empty() {
                trace!(
                    len = this.buf.len(),
                    cap = this.buf.capacity(),
                    "FramedWriter:poll_shutdown: write {}",
                    this.buf.len()
                );
                let n = ready!(this.inner.as_mut().poll_write(cx, &this.buf[..]))?;
                if n == 0 {
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "failed to write the buffer",
                    )));
                }
                this.buf.advance(n);
            }
        }
        *this.shutdown = true;
        ready!(this.inner.poll_flush(cx))?;
        trace!(
            len = self.buf.len(),
            cap = self.buf.capacity(),
            "FramedWriter:poll_shutdown: done"
        );
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod unittests {
    use std::time::Duration;

    use tokio::io::AsyncWriteExt;
    use tokio_test::io::Builder;

    use super::*;

    #[test_log::test(tokio::test)]
    async fn test_write_frames() {
        let mut mock = Builder::new()
            .write(b"\x01\0\0\0\0\0\0\0 \x04\0\0\0\0\0\0\0ABCD")
            .wait(Duration::from_millis(1))
            .write(b"\x01\0\0\0\0\0\0\0E\0\0\0")
            .wait(Duration::from_millis(1))
            .write(b"\0\0\0\0\0F")
            .build();
        let mut writer = FramedWriter::new(&mut mock);
        writer.write_all(b" ").await.unwrap();
        writer.flush().await.unwrap();
        writer.flush().await.unwrap();
        writer.flush().await.unwrap();
        writer.write_all(b"ABCD").await.unwrap();
        writer.flush().await.unwrap();
        writer.write_all(b"E").await.unwrap();
        writer.shutdown().await.unwrap();
        mock.write_all(b"F").await.unwrap();
    }
}
