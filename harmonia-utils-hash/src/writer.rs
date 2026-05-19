use crate::{Algorithm, Context, Hash};

/// A hash writer that implements [`AsyncWrite`].
///
/// # Examples
///
/// ```
/// use tokio::io;
/// use harmonia_utils_hash as hash;
///
/// # #[tokio::main]
/// # async fn main() -> std::io::Result<()> {
/// let mut reader: &[u8] = b"hello, world";
/// let mut writer = hash::HashWriter::new(hash::Algorithm::SHA256);
///
/// io::copy(&mut reader, &mut writer).await?;
/// let (size, hash) = writer.finish();
///
/// let one_shot = hash::Algorithm::SHA256.digest("hello, world");
/// assert_eq!(one_shot, hash);
/// assert_eq!(12, size);
/// # Ok(())
/// # }
/// ```
///
/// [`AsyncWrite`]: tokio::io::AsyncWrite
#[derive(Debug)]
pub struct HashWriter(Option<(u64, Context)>);

impl HashWriter {
    /// Constructs a new writer with `algorithm`.
    pub fn new(algorithm: Algorithm) -> HashWriter {
        HashWriter(Some((0, Context::new(algorithm))))
    }

    /// Finalizes this writer and returns the number of bytes written and the hash.
    pub fn finish(self) -> (u64, Hash) {
        let (read, ctx) = self.0.unwrap();
        (read, ctx.finish())
    }
}

impl tokio::io::AsyncWrite for HashWriter {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<Result<usize, std::io::Error>> {
        match self.0.as_mut() {
            None => {
                return std::task::Poll::Ready(Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "cannot write to `HashWriter` after calling `finish()`",
                )));
            }
            Some((read, ctx)) => {
                *read += buf.len() as u64;
                ctx.update(buf)
            }
        }
        std::task::Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        std::task::Poll::Ready(Ok(()))
    }
}
