#![allow(unsafe_code)]

use std::future::poll_fn;
use std::io::{self, Cursor};
use std::pin::Pin;
use std::task::{Poll, ready};

use bytes::{Buf, BufMut as _, Bytes};
use pin_project_lite::pin_project;
use tokio::io::{AsyncRead, ReadBuf};

use super::buffer::BufferMut;
use crate::AsyncBytesRead;

pub const DEFAULT_RESERVED_BUF_SIZE: usize = 8192;
pub const DEFAULT_MAX_BUF_SIZE: usize = 1024 * 1024 * 8;

pub struct BytesReaderBuilder {
    reserved_buf_size: usize,
    max_buf_size: usize,
}

impl Default for BytesReaderBuilder {
    fn default() -> Self {
        Self {
            reserved_buf_size: DEFAULT_RESERVED_BUF_SIZE,
            max_buf_size: DEFAULT_MAX_BUF_SIZE,
        }
    }
}

impl BytesReaderBuilder {
    pub fn set_reserved_buf_size(mut self, size: usize) -> Self {
        self.reserved_buf_size = size;
        self
    }

    pub fn set_max_buf_size(mut self, size: usize) -> Self {
        self.max_buf_size = size;
        self
    }

    pub fn build<R>(self, reader: R) -> BytesReader<R> {
        let buf = BufferMut::with_capacity(self.reserved_buf_size * 2);
        BytesReader {
            inner: reader,
            reserved_buf_size: self.reserved_buf_size,
            max_buf_size: self.max_buf_size,
            buf,
        }
    }
}

pin_project! {
    #[derive(Debug)]
    pub struct BytesReader<R> {
        #[pin]
        inner: R,
        buf: BufferMut,
        reserved_buf_size: usize,
        max_buf_size: usize,
    }
}

impl BytesReader<Cursor<Vec<u8>>> {
    pub fn builder() -> BytesReaderBuilder {
        BytesReaderBuilder::default()
    }
}

impl<R> BytesReader<R>
where
    R: AsyncRead,
{
    pub fn new(reader: R) -> BytesReader<R> {
        BytesReader::builder().build(reader)
    }

    fn poll_force_fill_buf_internal(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<std::io::Result<usize>> {
        // Ensure that buffer has space for at least reserved_buf_size bytes
        let reserved_buf_size = self.reserved_buf_size;
        if self.remaining_mut() < reserved_buf_size {
            self.as_mut().reserve(reserved_buf_size)?;
        }
        let me = self.project();
        let n = {
            let dst = me.buf.spare_capacity_mut();
            let mut buf = ReadBuf::uninit(dst);
            let ptr = buf.filled().as_ptr();
            ready!(me.inner.poll_read(cx, &mut buf)?);

            // Ensure the pointer does not change from under us
            assert_eq!(ptr, buf.filled().as_ptr());
            buf.filled().len()
        };

        // SAFETY: This is guaranteed to be the number of initialized (and read)
        // bytes due to the invariants provided by `ReadBuf::filled`.
        unsafe {
            me.buf.advance_mut(n);
        }
        Poll::Ready(Ok(n))
    }
}

impl<R> BytesReader<R>
where
    R: AsyncRead + Unpin,
{
    pub async fn force_fill(&mut self) -> io::Result<bytes::Bytes> {
        let mut p = Pin::new(self);
        let read = poll_fn(|cx| p.as_mut().poll_force_fill_buf(cx)).await?;
        Ok(read)
    }
}

impl<R> BytesReader<R> {
    pub fn get_ref(&self) -> &R {
        &self.inner
    }

    pub fn get_mut(&mut self) -> &mut R {
        &mut self.inner
    }

    pub fn get_pin_mut(self: Pin<&mut Self>) -> Pin<&mut R> {
        self.project().inner
    }

    pub fn into_inner(self) -> R {
        self.inner
    }

    pub fn buffer(&self) -> &[u8] {
        &self.buf
    }

    pub fn buffer_capacity(&self) -> usize {
        self.buf.capacity()
    }

    pub fn filled(&self) -> Bytes {
        self.buf.filled()
    }

    /// Remaining capacity in internal buffer
    pub fn remaining_mut(&self) -> usize {
        self.buf.capacity() - self.buf.len()
    }

    fn reserve(self: std::pin::Pin<&mut Self>, additional: usize) -> io::Result<()> {
        let me = self.project();
        me.buf.reserve(additional);
        if me.buf.capacity() > *me.max_buf_size {
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("buffer {} is too large", me.buf.capacity()),
            ))
        } else {
            Ok(())
        }
    }
}

impl<R> AsyncBytesRead for BytesReader<R>
where
    R: AsyncRead,
{
    fn poll_force_fill_buf(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<io::Result<bytes::Bytes>> {
        let read = ready!(self.as_mut().poll_force_fill_buf_internal(cx))?;
        if read == 0 {
            Poll::Ready(Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "EOF while force reading",
            )))
        } else {
            Poll::Ready(Ok(self.filled()))
        }
    }

    fn poll_fill_buf(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<std::io::Result<bytes::Bytes>> {
        if self.buf.is_empty() {
            ready!(self.as_mut().poll_force_fill_buf_internal(cx))?;
        }
        Poll::Ready(Ok(self.filled()))
    }

    fn prepare(self: std::pin::Pin<&mut Self>, additional: usize) {
        let me = self.project();
        let len = me.buf.len() + me.buf.len();
        if additional > len && me.buf.capacity() + additional < *me.max_buf_size {
            me.buf.reserve(additional - len);
        }
    }

    fn consume(self: std::pin::Pin<&mut Self>, amt: usize) {
        let me = self.project();
        me.buf.advance(amt);
    }
}

impl<R> AsyncRead for BytesReader<R>
where
    R: AsyncRead,
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

#[cfg(test)]
mod unittests {
    use hex_literal::hex;
    use tokio::io::AsyncReadExt as _;
    use tokio_test::io::Builder;

    use crate::{BytesReader, TryReadBytesLimited, TryReadU64};

    #[test_log::test(tokio::test)]
    async fn test_read_u64_partial() {
        let mock = Builder::new()
            .read(&hex!("0100 0000"))
            .read(&hex!("0000 0000 0123 4567 89AB CDEF"))
            .read(&hex!("0100 0000"))
            .build();
        let mut reader = BytesReader::new(mock);

        assert_eq!(
            1,
            TryReadU64::new().read(&mut reader).await.unwrap().unwrap()
        );
        assert_eq!(hex!("0123 4567 89AB CDEF"), reader.buffer());

        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).await.unwrap();
        assert_eq!(hex!("0123 4567 89AB CDEF 0100 0000"), &buf[..]);
    }

    #[test_log::test(tokio::test)]
    async fn test_read_twice() {
        let mock = Builder::new()
            .read(&hex!("0100 0000"))
            .read(&hex!("0000 0000 0123 4567 89AB CDEF"))
            .read(&hex!("0100 0000"))
            .build();
        let mut reader = BytesReader::new(mock);

        let mut buf = [0u8; 8];
        let n = reader.read(&mut buf).await.unwrap();
        assert_eq!(4, n);
        let n1 = reader.read(&mut buf[n..]).await.unwrap();
        assert_eq!(4, n1);
        assert_eq!(hex!("0100 0000 0000 0000"), buf);

        let mut buf = [0u8; 12];
        let n = reader.read(&mut buf).await.unwrap();
        assert_eq!(8, n);
        let n1 = reader.read(&mut buf[n..]).await.unwrap();
        assert_eq!(4, n1);
        assert_eq!(hex!("0123 4567 89AB CDEF 0100 0000"), buf);
        assert_eq!(0, reader.read(&mut buf[..]).await.unwrap());
    }

    #[test_log::test(tokio::test)]
    async fn test_force_fill() {
        let mock = Builder::new()
            .read(&hex!("0100"))
            .read(&hex!("0000 0000 0000"))
            .build();
        let mut reader = BytesReader::new(mock);

        //assert!(reader.filled().is_unique());
        let buf = reader.force_fill().await.unwrap();
        assert_eq!(2, buf.len());
        drop(buf);

        assert_eq!(8, reader.force_fill().await.unwrap().len());
    }

    #[tokio::test]
    async fn test_try_read_bytes_missing_padding() {
        let mock = Builder::new()
            .read(&hex!("0200 0000 0000 0000"))
            .read(&hex!("1234"))
            .build();
        let mut reader = BytesReader::new(mock);

        let ret = TryReadBytesLimited::new(0..=usize::MAX)
            .read(&mut reader)
            .await;

        assert_eq!(std::io::ErrorKind::UnexpectedEof, ret.unwrap_err().kind());
    }
}
