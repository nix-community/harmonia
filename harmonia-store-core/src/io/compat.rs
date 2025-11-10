use std::task::{Poll, ready};

use bytes::{Buf as _, Bytes};
use pin_project_lite::pin_project;
use tokio::io::{AsyncBufRead, AsyncRead};

use super::{AsyncBytesRead, DrainInto};

pin_project! {
    #[derive(Debug)]
    pub struct AsyncBufReadCompat<R> {
        #[pin]
        reader: R,
        buffer: Bytes,
    }
}

impl<R> AsyncBufReadCompat<R>
where
    R: AsyncBytesRead,
{
    pub fn new(reader: R) -> Self {
        Self {
            reader,
            buffer: Bytes::new(),
        }
    }
}

impl<R> AsyncRead for AsyncBufReadCompat<R>
where
    R: AsyncBytesRead,
{
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let rem = ready!(self.as_mut().poll_fill_buf(cx))?;
        if !rem.is_empty() {
            let amt = std::cmp::min(rem.len(), buf.remaining());
            buf.put_slice(&rem[0..amt]);
            self.consume(amt);
        }
        Poll::Ready(Ok(()))
    }
}

impl<R> AsyncBufRead for AsyncBufReadCompat<R>
where
    R: AsyncBytesRead,
{
    fn poll_fill_buf(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<std::io::Result<&[u8]>> {
        let this = self.project();
        if this.buffer.is_empty() {
            *this.buffer = ready!(this.reader.poll_fill_buf(cx))?;
        }
        Poll::Ready(Ok(&this.buffer[..]))
    }

    fn consume(self: std::pin::Pin<&mut Self>, amt: usize) {
        let this = self.project();
        this.buffer.advance(amt);
        if this.buffer.is_empty() {
            // Release the buffer when empty so that the reader can reclaim it
            // on the next call to poll_fill_buf
            *this.buffer = Bytes::new();
        }
        this.reader.consume(amt);
    }
}

impl<R, R2> DrainInto<R2> for AsyncBufReadCompat<R>
where
    R: DrainInto<R2>,
{
    fn poll_drain(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.project();
        *this.buffer = Bytes::new();
        this.reader.poll_drain(cx)
    }

    fn into_inner(self) -> R2 {
        self.reader.into_inner()
    }
}
