use std::future::{Future, poll_fn};
use std::io;
use std::mem::replace;
use std::pin::Pin;
use std::task::{Context, Poll, ready};

use bytes::Bytes;
use futures::FutureExt;
use pin_project_lite::pin_project;
use tokio::io::{AsyncBufRead, AsyncRead, ReadBuf};
use tokio::sync::oneshot;

use super::AsyncBytesRead;

pin_project! {
    struct Inner<R> {
        #[pin]
        reader: R,
        returner: oneshot::Sender<R>,
    }
}

pin_project! {
    #[derive(Debug)]
    pub struct Returner<R> {
        #[pin]
        inner: oneshot::Receiver<R>,
    }
}

impl<R> Future for Returner<R> {
    type Output = R;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let ret = ready!(self.project().inner.poll(cx));
        Poll::Ready(ret.expect("BUG: LentReader Sender was dropped without sending reader"))
    }
}

pub struct LentReader<R> {
    inner: Option<Inner<R>>,
}

impl<R> LentReader<R> {
    pub fn new(reader: R) -> (Returner<R>, LentReader<R>) {
        let (returner, receiver) = oneshot::channel();
        (
            Returner { inner: receiver },
            Self {
                inner: Some(Inner { reader, returner }),
            },
        )
    }
    fn return_reader(&mut self) {
        if let Some(inner) = self.inner.take() {
            let _ = inner.returner.send(inner.reader);
        }
    }
}

impl<R> Drop for LentReader<R> {
    fn drop(&mut self) {
        self.return_reader();
    }
}

impl<R> AsyncRead for LentReader<R>
where
    R: AsyncRead + Unpin,
{
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if let Some(inner) = this.inner.as_mut() {
            let filled = buf.filled().len();
            ready!(Pin::new(&mut inner.reader).poll_read(cx, buf))?;
            if filled == buf.filled().len() {
                this.return_reader();
            }
        }
        Poll::Ready(Ok(()))
    }
}

impl<R> AsyncBufRead for LentReader<R>
where
    R: AsyncBufRead + Unpin,
{
    fn poll_fill_buf(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<&[u8]>> {
        let this = self.get_mut();
        if let Some(inner) = this.inner.as_mut()
            && ready!(Pin::new(&mut inner.reader).poll_fill_buf(cx))?.is_empty()
        {
            this.return_reader();
        }
        if let Some(inner) = this.inner.as_mut() {
            let buf = ready!(Pin::new(&mut inner.reader).poll_fill_buf(cx))?;
            if !buf.is_empty() {
                return Poll::Ready(Ok(buf));
            }
        }
        Poll::Ready(Ok(&[]))
    }

    fn consume(self: Pin<&mut Self>, amt: usize) {
        let this = self.get_mut();
        if let Some(inner) = this.inner.as_mut() {
            Pin::new(&mut inner.reader).consume(amt);
        } else {
            assert!(amt == 0, "Non-zero consume on reader that is EOF");
        }
    }
}

impl<R> AsyncBytesRead for LentReader<R>
where
    R: AsyncBytesRead + Unpin,
{
    fn poll_force_fill_buf(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<Bytes>> {
        let this = self.get_mut();
        if let Some(inner) = this.inner.as_mut() {
            let buf = ready!(Pin::new(&mut inner.reader).poll_fill_buf(cx))?;
            if !buf.is_empty() {
                return Poll::Ready(Ok(buf));
            }
        }
        this.return_reader();
        Poll::Ready(Ok(Bytes::new()))
    }

    fn poll_fill_buf(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<Bytes>> {
        let this = self.get_mut();
        if let Some(inner) = this.inner.as_mut() {
            let buf = ready!(Pin::new(&mut inner.reader).poll_fill_buf(cx))?;
            if !buf.is_empty() {
                return Poll::Ready(Ok(buf));
            }
        }
        this.return_reader();
        Poll::Ready(Ok(Bytes::new()))
    }

    fn prepare(self: Pin<&mut Self>, additional: usize) {
        let this = self.get_mut();
        if let Some(inner) = this.inner.as_mut() {
            Pin::new(&mut inner.reader).prepare(additional);
        }
    }

    fn consume(self: Pin<&mut Self>, amt: usize) {
        let this = self.get_mut();
        if let Some(inner) = this.inner.as_mut() {
            Pin::new(&mut inner.reader).consume(amt);
        } else {
            assert!(amt == 0, "Non-zero consume on reader that is EOF");
        }
    }
}

pub trait DrainInto<R> {
    fn poll_drain(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>>;
    fn into_inner(self) -> R;
}

#[derive(Debug)]
pub enum Lending<R, W> {
    Lent(Returner<W>),
    Drain(Option<W>),
    Available(R),
}

impl<R, W> Lending<R, W>
where
    R: Unpin,
    W: DrainInto<R> + Unpin,
{
    pub fn new(reader: R) -> Self {
        Self::Available(reader)
    }

    pub fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        loop {
            match this {
                Self::Lent(returner) => {
                    let reader = ready!(returner.poll_unpin(cx));
                    *this = Self::Drain(Some(reader));
                }
                Self::Drain(reader) => {
                    ready!(
                        Pin::new(reader.as_mut().expect("BUG: Drain reader is None"))
                            .poll_drain(cx)
                    )?;
                    *this = Self::Available(reader.take().unwrap().into_inner());
                }
                Self::Available(_) => return Poll::Ready(Ok(())),
            }
        }
    }

    pub fn poll_reader(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<Pin<&mut R>>> {
        ready!(self.as_mut().poll_ready(cx))?;
        Poll::Ready(Ok(self.available_reader().unwrap()))
    }

    fn available_reader(self: Pin<&mut Self>) -> Option<Pin<&mut R>> {
        let this = self.get_mut();
        match this {
            Self::Available(reader) => Some(Pin::new(reader)),
            _ => None,
        }
    }

    pub async fn get_reader(&mut self) -> io::Result<&mut R> {
        let mut r = Pin::new(self);
        poll_fn(|cx| r.as_mut().poll_ready(cx)).await?;
        Ok(r.available_reader().unwrap().get_mut())
    }

    pub fn lend<F>(&mut self, f: F) -> LentReader<W>
    where
        F: FnOnce(R) -> W,
    {
        let (sender, receiver) = oneshot::channel();
        match replace(self, Self::Lent(Returner { inner: receiver })) {
            Self::Available(reader) => {
                let reader = f(reader);
                LentReader {
                    inner: Some(Inner {
                        reader,
                        returner: sender,
                    }),
                }
            }
            _ => panic!("trying to double lend reader"),
        }
    }
}

impl<R, W> AsyncRead for Lending<R, W>
where
    R: AsyncRead + Unpin,
    W: DrainInto<R> + Unpin,
{
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let reader = ready!(self.poll_reader(cx))?;
        reader.poll_read(cx, buf)
    }
}

impl<R, W> AsyncBytesRead for Lending<R, W>
where
    R: AsyncBytesRead + Unpin,
    W: DrainInto<R> + Unpin,
{
    fn poll_force_fill_buf(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<Bytes>> {
        let reader = ready!(self.poll_reader(cx))?;
        reader.poll_force_fill_buf(cx)
    }

    fn poll_fill_buf(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<Bytes>> {
        let reader = ready!(self.poll_reader(cx))?;
        reader.poll_fill_buf(cx)
    }

    fn prepare(self: Pin<&mut Self>, additional: usize) {
        let reader = self
            .available_reader()
            .expect("Reader must be available before prepare");
        reader.prepare(additional);
    }

    fn consume(self: Pin<&mut Self>, amt: usize) {
        let reader = self
            .available_reader()
            .expect("Reader must be available before consume");
        reader.consume(amt);
    }
}
