use std::io;
use std::ops::DerefMut;
use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::{Buf, Bytes};
use tokio::io::AsyncRead;

pub trait AsyncBytesRead: AsyncRead {
    fn poll_force_fill_buf(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<Bytes>>;

    fn poll_fill_buf(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<Bytes>>;

    fn prepare(self: Pin<&mut Self>, additional: usize);
    fn consume(self: Pin<&mut Self>, amt: usize);
}

macro_rules! deref_async_bytes_read {
    () => {
        fn poll_force_fill_buf(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<io::Result<Bytes>> {
            Pin::new(&mut **self.get_mut()).poll_force_fill_buf(cx)
        }

        fn poll_fill_buf(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<Bytes>> {
            Pin::new(&mut **self.get_mut()).poll_fill_buf(cx)
        }

        fn prepare(mut self: Pin<&mut Self>, additional: usize) {
            Pin::new(&mut **self).prepare(additional)
        }

        fn consume(mut self: Pin<&mut Self>, amt: usize) {
            Pin::new(&mut **self).consume(amt)
        }
    };
}

impl<T: ?Sized + AsyncBytesRead + Unpin> AsyncBytesRead for Box<T> {
    deref_async_bytes_read!();
}

impl<T: ?Sized + AsyncBytesRead + Unpin> AsyncBytesRead for &mut T {
    deref_async_bytes_read!();
}

impl<P> AsyncBytesRead for Pin<P>
where
    P: DerefMut + Unpin,
    P::Target: AsyncBytesRead,
{
    fn poll_force_fill_buf(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<Bytes>> {
        self.get_mut().as_mut().poll_force_fill_buf(cx)
    }

    fn poll_fill_buf(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<Bytes>> {
        self.get_mut().as_mut().poll_fill_buf(cx)
    }

    fn prepare(self: Pin<&mut Self>, additional: usize) {
        self.get_mut().as_mut().prepare(additional);
    }

    fn consume(self: Pin<&mut Self>, amt: usize) {
        self.get_mut().as_mut().consume(amt);
    }
}

impl AsyncBytesRead for io::Cursor<Bytes> {
    fn poll_force_fill_buf(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<Bytes>> {
        Poll::Ready(Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "bytes can't be force filled",
        )))
    }

    fn poll_fill_buf(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<Bytes>> {
        let pos = self.position();
        let mut buf = self.get_ref().clone();
        buf.advance(pos as usize);
        Poll::Ready(Ok(buf))
    }

    fn prepare(self: Pin<&mut Self>, _additional: usize) {}

    fn consume(self: Pin<&mut Self>, amt: usize) {
        io::BufRead::consume(self.get_mut(), amt);
    }
}
