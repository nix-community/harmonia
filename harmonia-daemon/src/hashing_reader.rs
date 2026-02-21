// SPDX-FileCopyrightText: 2025 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! An async reader adapter that computes a SHA-256 digest on the fly.
//!
//! Every byte read through this wrapper is fed into a [`ring::digest::Context`]
//! so that after the underlying reader is exhausted we can retrieve the hash
//! without ever buffering the full payload in memory.
//!
//! The digest state is kept behind an [`Arc<Mutex<…>>`] so that the caller
//! can extract the final hash even after the reader has been moved into a
//! consumer (e.g. `parse_nar`) that does not return it.

use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use pin_project_lite::pin_project;
use tokio::io::AsyncRead;

/// Shared accumulator for the incremental SHA-256 hash and byte count.
///
/// Wrapped in `Arc<Mutex<…>>` so both the [`HashingReader`] and its creator
/// can access the result after the reader is consumed.
pub struct HashState {
    ctx: ring::digest::Context,
    pub bytes_read: u64,
}

impl std::fmt::Debug for HashState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HashState")
            .field("bytes_read", &self.bytes_read)
            .finish_non_exhaustive()
    }
}

impl HashState {
    fn new() -> Self {
        Self {
            ctx: ring::digest::Context::new(&ring::digest::SHA256),
            bytes_read: 0,
        }
    }

    fn update(&mut self, data: &[u8]) {
        self.ctx.update(data);
        self.bytes_read += data.len() as u64;
    }

    /// Consume the state and return the final SHA-256 digest.
    pub fn finish(self) -> ring::digest::Digest {
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
    /// Create a new hashing reader.
    ///
    /// Returns the reader and a shared handle to the hash state.  The
    /// handle can be used to extract the digest after the reader has been
    /// fully consumed (even if the reader itself has been moved elsewhere).
    pub fn new(inner: R) -> (Self, Arc<Mutex<HashState>>) {
        let state = Arc::new(Mutex::new(HashState::new()));
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
        cx: &mut Context<'_>,
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
