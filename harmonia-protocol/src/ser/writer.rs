#![allow(unsafe_code)]

use std::fmt::{self, Write as _};
use std::future::poll_fn;
use std::io::{self, Cursor};
use std::ops::{Index, IndexMut, Range, RangeFull};
use std::pin::Pin;
use std::task::{Context, Poll, ready};

use bytes::{Buf, BufMut, BytesMut};
use pin_project_lite::pin_project;
use tokio::io::{AsyncWrite, AsyncWriteExt};

use crate::ProtocolVersion;
use harmonia_store_core::store_path::StoreDir;
use harmonia_utils_io::wire::{ZEROS, calc_padding};
use harmonia_utils_io::{DEFAULT_BUF_SIZE, RESERVED_BUF_SIZE};

use super::{Error, NixWrite};

pub struct NixWriterBuilder {
    buf: Option<BytesMut>,
    reserved_buf_size: usize,
    display_buf_size: usize,
    initial_buf_size: usize,
    version: ProtocolVersion,
    store_dir: StoreDir,
}

impl Default for NixWriterBuilder {
    fn default() -> Self {
        Self {
            buf: Default::default(),
            reserved_buf_size: RESERVED_BUF_SIZE,
            display_buf_size: 8192,
            initial_buf_size: DEFAULT_BUF_SIZE,
            version: Default::default(),
            store_dir: Default::default(),
        }
    }
}

impl NixWriterBuilder {
    pub fn set_buffer(mut self, buf: BytesMut) -> Self {
        self.buf = Some(buf);
        self
    }

    pub fn set_display_buf_size(mut self, size: usize) -> Self {
        assert!(
            size >= 8,
            "display_buf_size of {size} is to small to store u64",
        );
        self.display_buf_size = size;
        self
    }

    pub fn set_reserved_buf_size(mut self, size: usize) -> Self {
        assert!(
            size >= 8,
            "reserved_buf_size of {size} is to small to store u64"
        );
        self.reserved_buf_size = size;
        self
    }

    pub fn set_initial_buf_size(mut self, size: usize) -> Self {
        self.initial_buf_size = size;
        self
    }

    pub fn set_version(mut self, version: ProtocolVersion) -> Self {
        self.version = version;
        self
    }

    pub fn set_store_dir(mut self, store_dir: &StoreDir) -> Self {
        self.store_dir = store_dir.clone();
        self
    }

    pub fn build<W>(self, writer: W) -> NixWriter<W> {
        let buf = self
            .buf
            .unwrap_or_else(|| BytesMut::with_capacity(self.initial_buf_size));
        let buf = LimitBuffer(buf);
        NixWriter {
            buf,
            inner: writer,
            display_buf_size: self.display_buf_size,
            reserved_buf_size: self.reserved_buf_size,
            max_buf_size: self.initial_buf_size,
            version: self.version,
            store_dir: self.store_dir,
        }
    }
}

pin_project! {
    #[derive(Debug)]
    pub struct NixWriter<W> {
        #[pin]
        inner: W,
        buf: LimitBuffer,
        display_buf_size: usize,
        reserved_buf_size: usize,
        max_buf_size: usize,
        version: ProtocolVersion,
        store_dir: StoreDir,
    }
}

impl NixWriter<Cursor<Vec<u8>>> {
    pub fn builder() -> NixWriterBuilder {
        NixWriterBuilder::default()
    }
}

impl<W> NixWriter<W>
where
    W: AsyncWriteExt,
{
    pub fn new(writer: W) -> NixWriter<W> {
        NixWriter::builder().build(writer)
    }

    pub fn buffer(&self) -> &[u8] {
        &self.buf[..]
    }

    pub fn set_version(&mut self, version: ProtocolVersion) {
        self.version = version;
    }

    /// Remaining capacity in internal buffer
    pub fn remaining_mut(&self) -> usize {
        self.buf.capacity() - self.buf.len()
    }

    fn poll_flush_buf(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        let mut this = self.project();
        while !this.buf.is_empty() {
            let n = ready!(this.inner.as_mut().poll_write(cx, &this.buf[..]))?;
            if n == 0 {
                return Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "failed to write the buffer",
                )));
            }
            this.buf.advance(n);
        }
        let cap = this.buf.capacity();
        if cap < *this.reserved_buf_size {
            this.buf.reserve(*this.reserved_buf_size - cap);
        }
        Poll::Ready(Ok(()))
    }
}

impl<W> NixWriter<W>
where
    W: AsyncWriteExt + Unpin,
{
    async fn flush_buf(&mut self) -> Result<(), io::Error> {
        let mut s = Pin::new(self);
        poll_fn(move |cx| s.as_mut().poll_flush_buf(cx)).await
    }
}

impl<W> AsyncWrite for NixWriter<W>
where
    W: AsyncWrite,
{
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        // Flush when not enough space
        if self.remaining_mut() < buf.len() {
            ready!(self.as_mut().poll_flush_buf(cx))?;
        }
        let this = self.project();
        if buf.len() > this.buf.capacity() {
            this.inner.poll_write(cx, buf)
        } else {
            this.buf.put_slice(buf);
            Poll::Ready(Ok(buf.len()))
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        ready!(self.as_mut().poll_flush_buf(cx))?;
        self.project().inner.poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), io::Error>> {
        ready!(self.as_mut().poll_flush(cx))?;
        self.project().inner.poll_shutdown(cx)
    }
}

#[derive(Debug)]
struct LimitBuffer(BytesMut);
impl LimitBuffer {
    #[inline]
    fn len(&self) -> usize {
        self.0.len()
    }
    #[inline]
    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    #[inline]
    fn advance(&mut self, cnt: usize) {
        self.0.advance(cnt)
    }
    #[inline]
    fn truncate(&mut self, len: usize) {
        self.0.truncate(len)
    }
    #[inline]
    fn capacity(&self) -> usize {
        self.0.capacity()
    }
    #[inline]
    fn reserve(&mut self, additional: usize) {
        self.0.reserve(additional);
    }
}

impl fmt::Write for LimitBuffer {
    #[inline]
    fn write_str(&mut self, s: &str) -> fmt::Result {
        if self.remaining_mut() >= s.len() {
            self.put_slice(s.as_bytes());
            Ok(())
        } else {
            Err(fmt::Error)
        }
    }

    #[inline]
    fn write_fmt(&mut self, args: fmt::Arguments<'_>) -> fmt::Result {
        fmt::write(self, args)
    }
}
impl Index<RangeFull> for LimitBuffer {
    type Output = [u8];

    fn index(&self, index: RangeFull) -> &Self::Output {
        &self.0[index]
    }
}
impl Index<Range<usize>> for LimitBuffer {
    type Output = [u8];

    fn index(&self, index: Range<usize>) -> &Self::Output {
        &self.0[index]
    }
}
impl IndexMut<Range<usize>> for LimitBuffer {
    fn index_mut(&mut self, index: Range<usize>) -> &mut Self::Output {
        &mut self.0[index]
    }
}

unsafe impl BufMut for LimitBuffer {
    #[inline]
    fn remaining_mut(&self) -> usize {
        self.0.capacity() - self.0.len()
    }

    #[inline]
    unsafe fn advance_mut(&mut self, cnt: usize) {
        unsafe {
            self.0.advance_mut(cnt);
        }
    }

    #[inline]
    fn chunk_mut(&mut self) -> &mut bytes::buf::UninitSlice {
        self.0.spare_capacity_mut().into()
    }
}

impl<W> NixWrite for NixWriter<W>
where
    W: AsyncWrite + Send + Unpin,
{
    type Error = io::Error;

    fn version(&self) -> ProtocolVersion {
        self.version
    }

    fn store_dir(&self) -> &StoreDir {
        &self.store_dir
    }

    async fn write_number(&mut self, value: u64) -> Result<(), Self::Error> {
        self.write_all(&value.to_le_bytes()).await?;
        Ok(())
    }

    async fn write_slice(&mut self, buf: &[u8]) -> Result<(), Self::Error> {
        let padding = calc_padding(buf.len() as u64);
        self.write_value(&buf.len()).await?;
        self.write_all(buf).await?;
        if padding > 0 {
            self.write_all(&ZEROS[..padding]).await
        } else {
            Ok(())
        }
    }

    async fn write_display<D>(&mut self, msg: D) -> Result<(), Self::Error>
    where
        D: fmt::Display + Send,
        Self: Sized,
    {
        // Ensure that buffer has space for at least display_buf_size bytes
        if self.remaining_mut() < self.display_buf_size {
            self.flush_buf().await?;
        }
        let offset = self.buf.len();
        self.buf.put_u64_le(0);
        if let Err(err) = write!(self.buf, "{msg}") {
            self.buf.truncate(offset);
            return Err(Self::Error::unsupported_data(err));
        }
        let len = self.buf.len() - offset - 8;
        BufMut::put_u64_le(&mut &mut self.buf[offset..(offset + 8)], len as u64);
        let padding = calc_padding(len as u64);
        self.write_all(&ZEROS[..padding]).await?;
        Ok(())
    }
}

#[cfg(test)]
mod unittests {
    use std::{io::Cursor, time::Duration};

    use hex_literal::hex;
    use rstest::rstest;
    use tokio::io::AsyncWriteExt as _;
    use tokio_test::io::Builder;

    use crate::daemon::ser::{NixWrite, NixWriterBuilder};

    use super::NixWriter;

    #[tokio::test]
    async fn test_buffer_reclaim() {
        let under = Cursor::new(Vec::<u8>::new());
        let mut writer = NixWriterBuilder::default()
            .set_initial_buf_size(24)
            .set_reserved_buf_size(16)
            .build(under);
        assert_eq!(24, writer.remaining_mut());
        assert_eq!(24, writer.buf.capacity());
        writer.write_slice("1234567".as_bytes()).await.unwrap();
        assert_eq!(8, writer.remaining_mut());
        assert_eq!(24, writer.buf.capacity());
        writer.write_slice("1234567".as_bytes()).await.unwrap();
        assert_eq!(16, writer.remaining_mut());
        assert_eq!(24, writer.buf.capacity());
        writer.write_slice("1234567".as_bytes()).await.unwrap();
        assert_eq!(0, writer.remaining_mut());
        assert_eq!(24, writer.buf.capacity());
        writer.write_slice("1234567".as_bytes()).await.unwrap();
        assert_eq!(8, writer.remaining_mut());
        assert_eq!(24, writer.buf.capacity());
    }

    #[tokio::test]
    async fn test_reserve_buf() {
        let under = Cursor::new(Vec::<u8>::new());
        let mut writer = NixWriterBuilder::default()
            .set_initial_buf_size(2)
            .set_reserved_buf_size(9)
            .build(under);
        writer.write_slice("1234567".as_bytes()).await.unwrap();
        assert_eq!(1, writer.remaining_mut());
    }

    #[tokio::test]
    #[should_panic(expected = "reserved_buf_size of 7 is to small to store u64")]
    async fn test_invalid_reserved_buf_size() {
        NixWriterBuilder::default().set_reserved_buf_size(7);
    }

    #[tokio::test]
    #[should_panic(expected = "display_buf_size of 7 is to small to store u64")]
    async fn test_invalid_display_buf_size() {
        NixWriterBuilder::default().set_display_buf_size(7);
    }

    #[tokio::test]
    async fn test_display_error() {
        let under = Cursor::new(Vec::<u8>::new());
        let mut writer = NixWriterBuilder::default()
            .set_initial_buf_size(10)
            .set_reserved_buf_size(10)
            .set_display_buf_size(10)
            .build(under);
        writer.write_display("12").await.unwrap();
        assert_eq!(4, writer.remaining_mut());
        writer.write_display("123").await.unwrap_err();
        assert_eq!(10, writer.remaining_mut());
        assert_eq!(0, writer.buf.len());
    }

    #[rstest]
    #[case(1, &hex!("0100 0000 0000 0000"))]
    #[case::evil(666, &hex!("9A02 0000 0000 0000"))]
    #[case::max(u64::MAX, &hex!("FFFF FFFF FFFF FFFF"))]
    #[tokio::test]
    async fn test_write_number(#[case] number: u64, #[case] buf: &[u8]) {
        let mock = Builder::new().write(buf).build();
        let mut writer = NixWriter::new(mock);

        writer.write_number(number).await.unwrap();
        assert_eq!(writer.buffer(), buf);
        writer.flush().await.unwrap();
        assert_eq!(writer.buffer(), b"");
    }

    #[rstest]
    #[case::empty(b"", &hex!("0000 0000 0000 0000"))]
    #[case::one(b")", &hex!("0100 0000 0000 0000 2900 0000 0000 0000"))]
    #[case::two(b"it", &hex!("0200 0000 0000 0000 6974 0000 0000 0000"))]
    #[case::three(b"tea", &hex!("0300 0000 0000 0000 7465 6100 0000 0000"))]
    #[case::four(b"were", &hex!("0400 0000 0000 0000 7765 7265 0000 0000"))]
    #[case::five(b"where", &hex!("0500 0000 0000 0000 7768 6572 6500 0000"))]
    #[case::six(b"unwrap", &hex!("0600 0000 0000 0000 756E 7772 6170 0000"))]
    #[case::seven(b"where's", &hex!("0700 0000 0000 0000 7768 6572 6527 7300"))]
    #[case::aligned(b"read_tea", &hex!("0800 0000 0000 0000 7265 6164 5F74 6561"))]
    #[case::more_bytes(b"read_tess", &hex!("0900 0000 0000 0000 7265 6164 5F74 6573 7300 0000 0000 0000"))]
    #[tokio::test]
    async fn test_write_slice(
        #[case] value: &[u8],
        #[case] buf: &[u8],
        #[values(1, 2, 3, 4, 5, 6, 7, 8, 9, 1024)] chunks_size: usize,
        #[values(1, 2, 3, 4, 5, 6, 7, 8, 9, 1024)] buf_size: usize,
    ) {
        let mut builder = Builder::new();
        for chunk in buf.chunks(chunks_size) {
            builder.write(chunk);
            builder.wait(Duration::ZERO);
        }
        let mock = builder.build();
        let mut writer = NixWriter::builder()
            .set_initial_buf_size(buf_size)
            .build(mock);

        writer.write_slice(value).await.unwrap();
        //assert_eq!(writer.buffer(), buf);
        writer.flush().await.unwrap();
        assert_eq!(writer.buffer(), b"");
    }

    #[rstest]
    #[case::empty("", &hex!("0000 0000 0000 0000"))]
    #[case::one(")", &hex!("0100 0000 0000 0000 2900 0000 0000 0000"))]
    #[case::two("it", &hex!("0200 0000 0000 0000 6974 0000 0000 0000"))]
    #[case::three("tea", &hex!("0300 0000 0000 0000 7465 6100 0000 0000"))]
    #[case::four("were", &hex!("0400 0000 0000 0000 7765 7265 0000 0000"))]
    #[case::five("where", &hex!("0500 0000 0000 0000 7768 6572 6500 0000"))]
    #[case::six("unwrap", &hex!("0600 0000 0000 0000 756E 7772 6170 0000"))]
    #[case::seven("where's", &hex!("0700 0000 0000 0000 7768 6572 6527 7300"))]
    #[case::aligned("read_tea", &hex!("0800 0000 0000 0000 7265 6164 5F74 6561"))]
    #[case::more_bytes("read_tess", &hex!("0900 0000 0000 0000 7265 6164 5F74 6573 7300 0000 0000 0000"))]
    #[tokio::test]
    async fn test_write_display(
        #[case] value: &str,
        #[case] buf: &[u8],
        #[values(1, 2, 3, 4, 5, 6, 7, 8, 9, 1024)] chunks_size: usize,
    ) {
        let mut builder = Builder::new();
        for chunk in buf.chunks(chunks_size) {
            builder.write(chunk);
            builder.wait(Duration::ZERO);
        }
        let mock = builder.build();
        let mut writer = NixWriter::builder().build(mock);

        writer.write_display(value).await.unwrap();
        //assert_eq!(writer.buffer(), buf);
        writer.flush().await.unwrap();
        assert_eq!(writer.buffer(), b"");
    }
}
