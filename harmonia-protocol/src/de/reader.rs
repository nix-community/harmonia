use std::fmt::Debug;
use std::future::poll_fn;
use std::io::{self, Cursor};
use std::ops::RangeInclusive;
use std::pin::Pin;
use std::task::{Context, Poll, ready};

use bytes::Bytes;
use pin_project_lite::pin_project;
use tokio::io::{AsyncRead, ReadBuf};

use crate::daemon::ProtocolVersion;
use crate::io::{
    AsyncBytesRead, BytesReader, DEFAULT_MAX_BUF_SIZE, DEFAULT_RESERVED_BUF_SIZE,
    TryReadBytesLimited, TryReadU64,
};
use crate::store_path::StoreDir;

use super::NixRead;

pub struct NixReaderBuilder {
    reserved_buf_size: usize,
    max_buf_size: usize,
    version: ProtocolVersion,
    store_dir: StoreDir,
}

impl Default for NixReaderBuilder {
    fn default() -> Self {
        Self {
            reserved_buf_size: DEFAULT_RESERVED_BUF_SIZE,
            max_buf_size: DEFAULT_MAX_BUF_SIZE,
            version: Default::default(),
            store_dir: Default::default(),
        }
    }
}

impl NixReaderBuilder {
    pub fn set_reserved_buf_size(mut self, size: usize) -> Self {
        self.reserved_buf_size = size;
        self
    }

    pub fn set_max_buf_size(mut self, size: usize) -> Self {
        self.max_buf_size = size;
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

    pub fn build<R>(self, reader: R) -> NixReader<R>
    where
        R: AsyncBytesRead,
    {
        NixReader {
            inner: reader,
            reserved_buf_size: self.reserved_buf_size,
            max_buf_size: self.max_buf_size,
            version: self.version,
            store_dir: self.store_dir,
        }
    }

    pub fn build_buffered<R>(self, reader: R) -> NixReader<BytesReader<R>>
    where
        R: AsyncRead,
    {
        let reader = BytesReader::builder()
            .set_reserved_buf_size(self.reserved_buf_size)
            .set_max_buf_size(self.max_buf_size)
            .build(reader);
        self.build(reader)
    }
}

pin_project! {
    #[derive(Debug)]
    pub struct NixReader<R> {
        #[pin]
        inner: R,
        reserved_buf_size: usize,
        max_buf_size: usize,
        version: ProtocolVersion,
        store_dir: StoreDir,
    }
}

impl NixReader<Cursor<Vec<u8>>> {
    pub fn builder() -> NixReaderBuilder {
        NixReaderBuilder::default()
    }
}

impl<R> NixReader<BytesReader<R>>
where
    R: AsyncRead,
{
    pub fn new(reader: R) -> Self {
        NixReader::builder().build_buffered(reader)
    }
}

impl<R> NixReader<R> {
    pub fn set_version(&mut self, version: ProtocolVersion) {
        self.version = version;
    }

    pub fn get_ref(&self) -> &R {
        &self.inner
    }
    pub fn get_mut(&mut self) -> &mut R {
        &mut self.inner
    }
}

impl<R> NixReader<R>
where
    R: AsyncBytesRead + Unpin,
{
    pub async fn force_fill(&mut self) -> io::Result<Bytes> {
        let mut p = Pin::new(self);
        let read = poll_fn(|cx| p.as_mut().poll_force_fill_buf(cx)).await?;
        Ok(read)
    }
}

impl<R> NixRead for NixReader<R>
where
    R: AsyncBytesRead + Send + Unpin,
{
    type Error = io::Error;

    fn version(&self) -> ProtocolVersion {
        self.version
    }

    fn store_dir(&self) -> &StoreDir {
        &self.store_dir
    }

    async fn try_read_number(&mut self) -> Result<Option<u64>, Self::Error> {
        TryReadU64::new().read(self).await
    }

    async fn try_read_bytes_limited(
        &mut self,
        limit: RangeInclusive<usize>,
    ) -> Result<Option<Bytes>, Self::Error> {
        assert!(
            *limit.end() <= self.max_buf_size,
            "The limit must be smaller than {}",
            self.max_buf_size
        );
        TryReadBytesLimited::new(limit.clone()).read(self).await
    }

    async fn try_read_bytes(&mut self) -> Result<Option<Bytes>, Self::Error> {
        self.try_read_bytes_limited(0..=self.max_buf_size).await
    }

    async fn read_bytes(&mut self) -> Result<Bytes, Self::Error> {
        self.read_bytes_limited(0..=self.max_buf_size).await
    }
}

impl<R: AsyncBytesRead> AsyncRead for NixReader<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let rem = ready!(self.as_mut().poll_fill_buf(cx))?;
        if !rem.is_empty() {
            let amt = std::cmp::min(rem.len(), buf.remaining());
            buf.put_slice(&rem[0..amt]);
            self.consume(amt);
        }
        Poll::Ready(Ok(()))
    }
}

impl<R: AsyncBytesRead> AsyncBytesRead for NixReader<R> {
    fn poll_fill_buf(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<Bytes>> {
        self.project().inner.poll_fill_buf(cx)
    }

    fn consume(self: Pin<&mut Self>, amt: usize) {
        self.project().inner.consume(amt)
    }

    fn poll_force_fill_buf(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<Bytes>> {
        self.project().inner.poll_force_fill_buf(cx)
    }

    fn prepare(self: Pin<&mut Self>, additional: usize) {
        self.project().inner.prepare(additional)
    }
}

#[cfg(test)]
mod unittests {
    use std::{collections::BTreeSet, time::Duration};

    use hex_literal::hex;
    use rstest::rstest;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _, simplex};
    use tokio_test::io::Builder;

    use super::*;
    use crate::{
        btree_set,
        daemon::{
            de::NixRead,
            ser::{NixWrite, NixWriter},
        },
        hash::NarHash,
        io::BytesReader,
    };

    #[tokio::test]
    async fn test_read_u64() {
        let mock = Builder::new().read(&hex!("0100 0000 0000 0000")).build();
        let reader = BytesReader::new(mock);
        let mut reader = NixReader::new(reader);

        assert_eq!(1, reader.read_number().await.unwrap());
        assert_eq!(hex!(""), reader.get_ref().buffer());

        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).await.unwrap();
        assert_eq!(hex!(""), &buf[..]);
    }

    #[tokio::test]
    async fn test_read_u64_rest() {
        let mock = Builder::new()
            .read(&hex!("0100 0000 0000 0000 0123 4567 89AB CDEF"))
            .build();
        let mut reader = NixReader::new(mock);

        assert_eq!(1, reader.read_number().await.unwrap());
        assert_eq!(hex!("0123 4567 89AB CDEF"), reader.get_ref().buffer());

        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).await.unwrap();
        assert_eq!(hex!("0123 4567 89AB CDEF"), &buf[..]);
    }

    #[tokio::test]
    async fn test_read_u64_partial() {
        let mock = Builder::new()
            .read(&hex!("0100 0000"))
            .wait(Duration::ZERO)
            .read(&hex!("0000 0000 0123 4567 89AB CDEF"))
            .wait(Duration::ZERO)
            .read(&hex!("0100 0000"))
            .build();
        let mut reader = NixReader::new(mock);

        assert_eq!(1, reader.read_number().await.unwrap());
        assert_eq!(hex!("0123 4567 89AB CDEF"), reader.get_ref().buffer());

        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).await.unwrap();
        assert_eq!(hex!("0123 4567 89AB CDEF 0100 0000"), &buf[..]);
    }

    #[tokio::test]
    async fn test_read_u64_eof() {
        let mock = Builder::new().build();
        let mut reader = NixReader::new(mock);

        assert_eq!(
            io::ErrorKind::UnexpectedEof,
            reader.read_number().await.unwrap_err().kind()
        );
    }

    #[tokio::test]
    async fn test_try_read_u64_none() {
        let mock = Builder::new().build();
        let mut reader = NixReader::new(mock);

        assert_eq!(None, reader.try_read_number().await.unwrap());
    }

    #[tokio::test]
    async fn test_try_read_u64_eof() {
        let mock = Builder::new().read(&hex!("0100 0000 0000")).build();
        let mut reader = NixReader::new(mock);

        assert_eq!(
            io::ErrorKind::UnexpectedEof,
            reader.try_read_number().await.unwrap_err().kind()
        );
    }

    #[tokio::test]
    async fn test_try_read_u64_eof2() {
        let mock = Builder::new()
            .read(&hex!("0100"))
            .wait(Duration::ZERO)
            .read(&hex!("0000 0000"))
            .build();
        let mut reader = NixReader::new(mock);

        assert_eq!(
            io::ErrorKind::UnexpectedEof,
            reader.try_read_number().await.unwrap_err().kind()
        );
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
    async fn test_read_bytes(#[case] expected: &[u8], #[case] data: &[u8]) {
        let mock = Builder::new().read(data).build();
        let mut reader = NixReader::new(mock);
        let actual = reader.read_bytes().await.unwrap();
        assert_eq!(&actual[..], expected);
    }

    #[tokio::test]
    async fn test_read_bytes_empty() {
        let mock = Builder::new().build();
        let mut reader = NixReader::new(mock);

        assert_eq!(
            io::ErrorKind::UnexpectedEof,
            reader.read_bytes().await.unwrap_err().kind()
        );
    }

    #[tokio::test]
    async fn test_try_read_bytes_none() {
        let mock = Builder::new().build();
        let mut reader = NixReader::new(mock);

        assert_eq!(None, reader.try_read_bytes().await.unwrap());
    }

    #[tokio::test]
    async fn test_try_read_bytes_missing_data() {
        let mock = Builder::new()
            .read(&hex!("0500"))
            .wait(Duration::ZERO)
            .read(&hex!("0000 0000"))
            .build();
        let mut reader = NixReader::new(mock);

        assert_eq!(
            io::ErrorKind::UnexpectedEof,
            reader.try_read_bytes().await.unwrap_err().kind()
        );
    }

    #[tokio::test]
    async fn test_try_read_bytes_missing_padding() {
        let mock = Builder::new()
            .read(&hex!("0200 0000 0000 0000"))
            .wait(Duration::ZERO)
            .read(&hex!("1234"))
            .build();
        let mut reader = NixReader::new(mock);

        assert_eq!(
            io::ErrorKind::UnexpectedEof,
            reader.try_read_bytes().await.unwrap_err().kind()
        );
    }

    #[tokio::test]
    async fn test_read_bytes_bad_padding() {
        let mock = Builder::new()
            .read(&hex!("0200 0000 0000 0000"))
            .wait(Duration::ZERO)
            .read(&hex!("1234 0100 0000 0000"))
            .build();
        let mut reader = NixReader::new(mock);

        assert_eq!(
            io::ErrorKind::InvalidData,
            reader.read_bytes().await.unwrap_err().kind()
        );
    }

    #[tokio::test]
    async fn test_read_bytes_limited_out_of_range() {
        let mock = Builder::new().read(&hex!("FFFF 0000 0000 0000")).build();
        let mut reader = NixReader::new(mock);

        assert_eq!(
            io::ErrorKind::InvalidData,
            reader.read_bytes_limited(0..=50).await.unwrap_err().kind()
        );
    }

    #[tokio::test]
    async fn test_read_bytes_length_overflow() {
        let mock = Builder::new().read(&hex!("F9FF FFFF FFFF FFFF")).build();
        let mut reader = NixReader::builder()
            .set_max_buf_size(usize::MAX)
            .build_buffered(mock);

        assert_eq!(
            io::ErrorKind::InvalidData,
            reader
                .read_bytes_limited(0..=usize::MAX)
                .await
                .unwrap_err()
                .kind()
        );
    }

    // FUTURE: Test this on supported hardware
    #[tokio::test]
    #[cfg(any(target_pointer_width = "16", target_pointer_width = "32"))]
    async fn test_bytes_length_conversion_overflow() {
        let len = (usize::MAX as u64) + 1;
        let mock = Builder::new().read(&len.to_le_bytes()).build();
        let mut reader = NixReader::new(mock);

        assert_eq!(
            std::io::ErrorKind::InvalidData,
            reader.read_value::<usize>().await.unwrap_err().kind()
        );
    }

    // FUTURE: Test this on supported hardware
    #[tokio::test]
    #[cfg(any(target_pointer_width = "16", target_pointer_width = "32"))]
    async fn test_bytes_aligned_length_conversion_overflow() {
        let len = (usize::MAX - 6) as u64;
        let mock = Builder::new().read(&len.to_le_bytes()).build();
        let mut reader = NixReader::new(mock);

        assert_eq!(
            std::io::ErrorKind::InvalidData,
            reader.read_value::<usize>().await.unwrap_err().kind()
        );
    }

    #[tokio::test]
    async fn test_query_info() {
        let (reader, writer) = simplex(DEFAULT_RESERVED_BUF_SIZE);
        let mut writer = NixWriter::new(writer);
        let mut reader = NixReader::new(reader);
        writer.write_number(12).await.unwrap();
        let value = crate::daemon::UnkeyedValidPathInfo {
            deriver: Some("00000000000000000000000000000000-_.drv".parse().unwrap()),
            nar_hash: NarHash::new(&[0u8; 32]),
            references: btree_set!["00000000000000000000000000000000-_"],
            registration_time: 0,
            nar_size: 0,
            ultimate: true,
            signatures: BTreeSet::new(),
            ca: None,
        };
        writer.write_value(&value).await.unwrap();
        writer.write_number(14).await.unwrap();
        writer.flush().await.unwrap();
        assert_eq!(12, reader.read_number().await.unwrap());
        assert_eq!(value, reader.read_value().await.unwrap());
        assert_eq!(14, reader.read_number().await.unwrap());
    }
}
