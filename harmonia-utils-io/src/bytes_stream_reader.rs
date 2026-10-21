use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, ready};

use bytes::{Buf as _, BufMut as _, Bytes};
use futures_util::stream::{Fuse, Stream, StreamExt as _};
use pin_project_lite::pin_project;
use tokio::io::{AsyncRead, ReadBuf};

use crate::AsyncBytesRead;
use crate::bytes_reader::BufferMut;

const MIN_JOIN: usize = 4096;

pin_project! {
    /// An [`AsyncBytesRead`] over a stream of [`Bytes`] chunks. Only
    /// `poll_force_fill_buf` copies, to join the end of one chunk with the
    /// start of the next.
    #[derive(Debug)]
    pub struct BytesStreamReader<S> {
        #[pin]
        stream: Fuse<S>,
        buf: Bytes,
        // When set, `buf` is a view of it, and force fills append to it.
        join: Option<BufferMut>,
        // The part of a chunk that a join didn't copy.
        rest: Bytes,
    }
}

impl<S: Stream> BytesStreamReader<S> {
    pub fn new(stream: S) -> Self {
        Self {
            stream: stream.fuse(),
            buf: Bytes::new(),
            join: None,
            rest: Bytes::new(),
        }
    }
}

impl<S> BytesStreamReader<S>
where
    S: Stream<Item = io::Result<Bytes>>,
{
    /// Returns the bytes after `buf`, empty at the end of the stream.
    fn poll_next_chunk(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<Bytes>> {
        let mut this = self.project();
        if !this.rest.is_empty() {
            return Poll::Ready(Ok(std::mem::take(this.rest)));
        }
        loop {
            match ready!(this.stream.as_mut().poll_next(cx)) {
                Some(Ok(chunk)) if chunk.is_empty() => {}
                Some(chunk) => return Poll::Ready(chunk),
                None => return Poll::Ready(Ok(Bytes::new())),
            }
        }
    }
}

impl<S> AsyncBytesRead for BytesStreamReader<S>
where
    S: Stream<Item = io::Result<Bytes>>,
{
    fn poll_force_fill_buf(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<Bytes>> {
        let mut chunk = ready!(self.as_mut().poll_next_chunk(cx))?;
        if chunk.is_empty() {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "EOF while force reading",
            )));
        }
        let this = self.project();
        if this.buf.is_empty() {
            *this.buf = chunk;
            return Poll::Ready(Ok(this.buf.clone()));
        }
        let join = match this.join {
            Some(join) if join.remaining_mut() > 0 => join,
            // Grow geometrically so tiny chunks aren't copied once each.
            full => {
                let len = this.buf.len();
                let room = if full.is_some() {
                    len.max(MIN_JOIN)
                } else {
                    MIN_JOIN
                };
                let mut join = BufferMut::with_capacity(len + room);
                join.put_slice(this.buf);
                full.insert(join)
            }
        };
        let take = chunk.len().min(join.remaining_mut());
        join.put_slice(&chunk[..take]);
        chunk.advance(take);
        *this.rest = chunk;
        *this.buf = join.filled();
        Poll::Ready(Ok(this.buf.clone()))
    }

    fn poll_fill_buf(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<Bytes>> {
        if self.buf.is_empty() {
            let chunk = ready!(self.as_mut().poll_next_chunk(cx))?;
            *self.as_mut().project().buf = chunk;
        }
        Poll::Ready(Ok(self.buf.clone()))
    }

    fn prepare(self: Pin<&mut Self>, _additional: usize) {}

    fn consume(self: Pin<&mut Self>, amt: usize) {
        let this = self.project();
        this.buf.advance(amt);
        if this.buf.is_empty() {
            // Drop the chunk so that its producer can reuse the buffer.
            *this.buf = Bytes::new();
            *this.join = None;
        } else if let Some(join) = this.join {
            join.advance(amt);
        }
    }
}

impl<S> AsyncRead for BytesStreamReader<S>
where
    S: Stream<Item = io::Result<Bytes>>,
{
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let rem = ready!(self.as_mut().poll_fill_buf(cx))?;
        let amt = rem.len().min(buf.remaining());
        buf.put_slice(&rem[..amt]);
        self.consume(amt);
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod unittests {
    use std::future::poll_fn;
    use std::io;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    use bytes::{Buf as _, Bytes};
    use futures_util::stream::{self, Stream};
    use proptest::prelude::*;
    use proptest::sample::Index;

    use super::BytesStreamReader;
    use crate::AsyncBytesRead;

    async fn fill<R: AsyncBytesRead + Unpin>(reader: &mut R) -> io::Result<Bytes> {
        poll_fn(|cx| Pin::new(&mut *reader).poll_fill_buf(cx)).await
    }

    async fn force_fill<R: AsyncBytesRead + Unpin>(reader: &mut R) -> io::Result<Bytes> {
        poll_fn(|cx| Pin::new(&mut *reader).poll_force_fill_buf(cx)).await
    }

    fn chunks(data: &[u8], sizes: &[usize]) -> Vec<io::Result<Bytes>> {
        let mut data = Bytes::copy_from_slice(data);
        let mut chunks = Vec::new();
        for size in sizes.iter().cycle() {
            if data.is_empty() {
                break;
            }
            chunks.push(Ok(data.split_to((*size).min(data.len()))));
        }
        chunks
    }

    /// Returns `Pending` before every item.
    struct Stalling<S> {
        inner: S,
        stalled: bool,
    }

    impl<S: Stream + Unpin> Stream for Stalling<S> {
        type Item = S::Item;

        fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<S::Item>> {
            self.stalled = !self.stalled;
            if self.stalled {
                cx.waker().wake_by_ref();
                return Poll::Pending;
            }
            Pin::new(&mut self.inner).poll_next(cx)
        }
    }

    #[derive(Debug, Clone)]
    enum Op {
        Fill,
        ForceFill,
        Consume(Index),
    }

    #[test]
    fn proptest_buffer_matches_stream() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        proptest!(|(
            data in prop::collection::vec(any::<u8>(), 0..512),
            sizes in prop::collection::vec(1..=64usize, 1..8),
            ops in prop::collection::vec(prop_oneof![
                Just(Op::Fill),
                Just(Op::ForceFill),
                any::<Index>().prop_map(Op::Consume),
            ], 0..64),
        )| {
            rt.block_on(async {
                let source = stream::iter(chunks(&data, &sizes));
                let mut reader = BytesStreamReader::new(Stalling { inner: source, stalled: false });
                let mut pos = 0;
                let mut held = Bytes::new();
                for op in ops {
                    match op {
                        Op::Fill => {
                            held = fill(&mut reader).await?;
                            prop_assert!(!held.is_empty() || pos == data.len());
                        }
                        Op::ForceFill => match force_fill(&mut reader).await {
                            Ok(buf) => {
                                prop_assert!(buf.len() > held.len());
                                held = buf;
                            }
                            Err(err) => {
                                prop_assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
                                prop_assert_eq!(pos + held.len(), data.len());
                            }
                        },
                        Op::Consume(amt) => {
                            let amt = amt.index(held.len() + 1);
                            Pin::new(&mut reader).consume(amt);
                            held.advance(amt);
                            pos += amt;
                        }
                    }
                    prop_assert_eq!(&held[..], &data[pos..pos + held.len()]);
                }
                Ok::<_, TestCaseError>(())
            })?;
        });
    }
}
