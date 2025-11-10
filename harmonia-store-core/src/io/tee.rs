use std::cmp::min;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, ready};

use pin_project_lite::pin_project;
use tokio::io::AsyncWrite;

use crate::io::DEFAULT_BUF_SIZE;

pin_project! {
    pub struct TeeWriter<W1, W2> {
        buf: Vec<u8>,
        written1: usize,
        written2: usize,
        #[pin]
        writer1: W1,
        #[pin]
        writer2: W2,
    }
}

impl<W1, W2> TeeWriter<W1, W2>
where
    W1: AsyncWrite,
    W2: AsyncWrite,
{
    pub fn new(writer1: W1, writer2: W2) -> Self {
        Self {
            buf: Vec::with_capacity(DEFAULT_BUF_SIZE),
            writer1,
            writer2,
            written1: 0,
            written2: 0,
        }
    }

    fn poll_flush_buf(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if !self.buf.is_empty() {
            while self.written1 < self.buf.len() || self.written2 < self.buf.len() {
                let this = self.as_mut().project();
                if *this.written1 < this.buf.len() {
                    let n = ready!(this.writer1.poll_write(cx, &this.buf[*this.written1..]))?;
                    *this.written1 += n;
                }
                if *this.written2 < this.buf.len() {
                    let n = ready!(this.writer2.poll_write(cx, &this.buf[*this.written2..]))?;
                    *this.written2 += n;
                }
            }
            let this = self.project();
            this.buf.clear();
            *this.written1 = 0;
            *this.written2 = 0;
        }
        Poll::Ready(Ok(()))
    }
}

impl<W1, W2> AsyncWrite for TeeWriter<W1, W2>
where
    W1: AsyncWrite,
    W2: AsyncWrite,
{
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        if buf.len() < self.buf.capacity() {
            if buf.len() + self.buf.len() > self.buf.capacity() {
                ready!(self.as_mut().poll_flush_buf(cx))?;
            }
            let this = self.project();
            let rem = min(buf.len(), this.buf.capacity() - this.buf.len());
            this.buf.extend_from_slice(&buf[..rem]);
            Poll::Ready(Ok(rem))
        } else {
            let this = self.project();
            let rem = min(buf.len(), this.buf.capacity() - this.buf.len());
            this.buf.extend_from_slice(&buf[..rem]);
            Poll::Ready(Ok(rem))
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        ready!(self.as_mut().poll_flush_buf(cx))?;
        let this = self.project();
        ready!(this.writer1.poll_flush(cx))?;
        this.writer2.poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        ready!(self.as_mut().poll_flush(cx))?;
        let this = self.project();
        ready!(this.writer1.poll_shutdown(cx))?;
        this.writer2.poll_shutdown(cx)
    }
}
