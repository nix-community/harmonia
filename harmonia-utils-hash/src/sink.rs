use std::fmt as sfmt;

use sha1::Digest;

use crate::{Algorithm, Hash};

#[derive(Clone)]
enum InnerContext {
    MD5(md5::Context),
    SHA1(sha1::Sha1),
    SHA256(sha2::Sha256),
    SHA512(sha2::Sha512),
    BLAKE3(Box<blake3::Hasher>),
}

/// A context for multi-step (Init-Update-Finish) digest calculation.
///
/// # Examples
///
/// ```
/// use harmonia_utils_hash as hash;
///
/// let one_shot = hash::Algorithm::SHA256.digest("hello, world");
///
/// let mut ctx = hash::Context::new(hash::Algorithm::SHA256);
/// ctx.update("hello");
/// ctx.update(", ");
/// ctx.update("world");
/// let multi_path = ctx.finish();
///
/// assert_eq!(one_shot, multi_path);
/// ```
#[derive(Clone)]
pub struct Context(Algorithm, InnerContext);

impl Context {
    /// Constructs a new context with `algorithm`.
    pub fn new(algorithm: Algorithm) -> Self {
        let inner = match algorithm {
            Algorithm::MD5 => InnerContext::MD5(md5::Context::new()),
            Algorithm::SHA1 => InnerContext::SHA1(sha1::Sha1::new()),
            Algorithm::SHA256 => InnerContext::SHA256(sha2::Sha256::new()),
            Algorithm::SHA512 => InnerContext::SHA512(sha2::Sha512::new()),
            Algorithm::BLAKE3 => InnerContext::BLAKE3(Box::new(blake3::Hasher::new())),
        };
        Context(algorithm, inner)
    }

    /// Update the digest with all the data in `data`.
    /// `update` may be called zero or more times before `finish` is called.
    pub fn update<D: AsRef<[u8]>>(&mut self, data: D) {
        let data = data.as_ref();
        match &mut self.1 {
            InnerContext::MD5(ctx) => ctx.consume(data),
            InnerContext::SHA1(ctx) => ctx.update(data),
            InnerContext::SHA256(ctx) => ctx.update(data),
            InnerContext::SHA512(ctx) => ctx.update(data),
            InnerContext::BLAKE3(ctx) => {
                ctx.update(data);
            }
        }
    }

    /// Finalizes the digest calculation and returns the [`Hash`] value.
    /// This consumes the context to prevent misuse.
    ///
    /// [`Hash`]: struct@Hash
    pub fn finish(self) -> Hash {
        match self.1 {
            InnerContext::MD5(ctx) => Hash::new(self.0, ctx.finalize().as_ref()),
            InnerContext::SHA1(ctx) => Hash::new(self.0, ctx.finalize().as_ref()),
            InnerContext::SHA256(ctx) => Hash::new(self.0, ctx.finalize().as_ref()),
            InnerContext::SHA512(ctx) => Hash::new(self.0, ctx.finalize().as_ref()),
            InnerContext::BLAKE3(ctx) => Hash::new(self.0, ctx.finalize().as_bytes()),
        }
    }

    /// The algorithm that this context is using.
    pub fn algorithm(&self) -> Algorithm {
        self.0
    }
}

impl sfmt::Debug for Context {
    fn fmt(&self, f: &mut sfmt::Formatter<'_>) -> sfmt::Result {
        f.debug_tuple("Context").field(&self.0).finish()
    }
}

/// A hash sink that implements [`AsyncWrite`].
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
/// let mut sink = hash::HashSink::new(hash::Algorithm::SHA256);
///
/// io::copy(&mut reader, &mut sink).await?;
/// let (size, hash) = sink.finish();
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
pub struct HashSink(Option<(u64, Context)>);

impl HashSink {
    /// Constructs a new sink with `algorithm`.
    pub fn new(algorithm: Algorithm) -> HashSink {
        HashSink(Some((0, Context::new(algorithm))))
    }

    /// Finalizes this sink and returns the hash and number of bytes written to the sink.
    pub fn finish(self) -> (u64, Hash) {
        let (read, ctx) = self.0.unwrap();
        (read, ctx.finish())
    }
}

impl tokio::io::AsyncWrite for HashSink {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<Result<usize, std::io::Error>> {
        match self.0.as_mut() {
            None => {
                return std::task::Poll::Ready(Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "cannot write to HashSink after calling finish()",
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
