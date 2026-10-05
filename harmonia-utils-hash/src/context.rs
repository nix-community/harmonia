use std::fmt as sfmt;

use crate::sha::ShaContext;
use crate::{Algorithm, Hash};

#[derive(Clone)]
enum InnerContext {
    MD5(md5::Context),
    Sha(ShaContext),
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
            Algorithm::SHA1 | Algorithm::SHA256 | Algorithm::SHA512 => {
                InnerContext::Sha(ShaContext::new(algorithm))
            }
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
            InnerContext::Sha(ctx) => ctx.update(data),
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
            InnerContext::Sha(ctx) => Hash::new(self.0, ctx.finish().as_ref()),
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
