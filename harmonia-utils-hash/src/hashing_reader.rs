// SPDX-FileCopyrightText: 2026 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! An async reader adapter that computes a hash digest on the fly.
//!
//! Every byte read through this wrapper is fed into a [`Context`] so that
//! after the underlying reader is exhausted we can retrieve the hash without
//! ever buffering the full payload in memory.
//!
//! The digest state is kept behind an [`Arc<Mutex<…>>`] so that the caller
//! can extract the final hash even after the reader has been moved into a
//! consumer (e.g. `parse_nar`) that does not return it.

use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context as TaskContext, Poll};

use pin_project_lite::pin_project;
use tokio::io::AsyncRead;

use crate::{Algorithm, Context, Hash};

/// Shared accumulator for the incremental hash and byte count.
///
/// Wrapped in `Arc<Mutex<…>>` so both the [`HashingReader`] and its creator
/// can access the result after the reader is consumed.
pub struct HashState {
    ctx: Context,
    /// Total number of bytes that have been read through the reader.
    pub bytes_read: u64,
}

impl std::fmt::Debug for HashState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HashState")
            .field("algorithm", &self.ctx.algorithm())
            .field("bytes_read", &self.bytes_read)
            .finish_non_exhaustive()
    }
}

impl HashState {
    fn new(algorithm: Algorithm) -> Self {
        Self {
            ctx: Context::new(algorithm),
            bytes_read: 0,
        }
    }

    fn update(&mut self, data: &[u8]) {
        self.ctx.update(data);
        self.bytes_read += data.len() as u64;
    }

    /// Consume the state and return the final digest as a [`Hash`].
    pub fn finish(self) -> Hash {
        self.ctx.finish()
    }
}

pin_project! {
    /// Wraps an [`AsyncRead`] and incrementally hashes every byte that passes
    /// through.
    ///
    /// After the stream is exhausted, use the [`Arc<Mutex<HashState>>`]
    /// returned by [`new`](Self::new) to retrieve the digest via
    /// [`HashState::finish`].
    pub struct HashingReader<R> {
        #[pin]
        inner: R,
        state: Arc<Mutex<HashState>>,
    }
}

impl<R> HashingReader<R> {
    /// Create a new hashing reader that computes SHA-256.
    ///
    /// Returns the reader and a shared handle to the hash state. The
    /// handle can be used to extract the digest after the reader has been
    /// fully consumed (even if the reader itself has been moved elsewhere).
    pub fn new(inner: R) -> (Self, Arc<Mutex<HashState>>) {
        Self::with_algorithm(inner, Algorithm::SHA256)
    }

    /// Create a new hashing reader with a specific algorithm.
    ///
    /// Returns the reader and a shared handle to the hash state.
    pub fn with_algorithm(inner: R, algorithm: Algorithm) -> (Self, Arc<Mutex<HashState>>) {
        let state = Arc::new(Mutex::new(HashState::new(algorithm)));
        let reader = Self {
            inner,
            state: Arc::clone(&state),
        };
        (reader, state)
    }
}

impl<R: AsyncRead> AsyncRead for HashingReader<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.project();
        let before = buf.filled().len();
        let result = this.inner.poll_read(cx, buf);
        if let Poll::Ready(Ok(())) = &result {
            let new_bytes = &buf.filled()[before..];
            if !new_bytes.is_empty() {
                this.state.lock().unwrap().update(new_bytes);
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt as _;

    #[tokio::test]
    async fn test_hashing_reader_sha256() {
        let data = b"hello, world";
        let cursor = std::io::Cursor::new(data);
        let (mut reader, state) = HashingReader::new(cursor);

        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).await.unwrap();
        assert_eq!(buf, data);

        drop(reader);
        let hash_state = Arc::try_unwrap(state).unwrap().into_inner().unwrap();
        assert_eq!(hash_state.bytes_read, data.len() as u64);

        let hash = hash_state.finish();
        let expected = Algorithm::SHA256.digest(data);
        assert_eq!(hash, expected);
    }

    #[tokio::test]
    async fn test_hashing_reader_empty() {
        let data = b"";
        let cursor = std::io::Cursor::new(data);
        let (mut reader, state) = HashingReader::new(cursor);

        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).await.unwrap();
        assert!(buf.is_empty());

        drop(reader);
        let hash_state = Arc::try_unwrap(state).unwrap().into_inner().unwrap();
        assert_eq!(hash_state.bytes_read, 0);

        let hash = hash_state.finish();
        let expected = Algorithm::SHA256.digest(b"");
        assert_eq!(hash, expected);
    }
}
