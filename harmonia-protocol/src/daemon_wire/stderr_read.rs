use std::io;
use std::pin::Pin;
use std::task::{Poll, ready};

use bytes::{Buf, Bytes};
use pin_project_lite::pin_project;
use tokio::io::{AsyncBufRead, AsyncRead};
use tokio::sync::mpsc;
use tokio_util::sync::PollSender;

use crate::io::{AsyncBytesRead, DEFAULT_BUF_SIZE, TryReadBytesLimited};

enum State {
    Sending(usize),
    Reading(TryReadBytesLimited),
    Buffer(Bytes),
}

pin_project! {
    pub struct StderrReader<R> {
        inner: R,
        sender: PollSender<usize>,
        state: State,
    }
}

impl<R> StderrReader<R>
where
    R: AsyncBytesRead + Unpin,
{
    pub fn new(reader: R) -> (mpsc::Receiver<usize>, Self) {
        let (sender, receiver) = mpsc::channel(1);
        (
            receiver,
            StderrReader {
                sender: PollSender::new(sender),
                inner: reader,
                state: State::Buffer(Bytes::new()),
            },
        )
    }
}

impl<R> AsyncRead for StderrReader<R>
where
    R: AsyncBytesRead + Unpin,
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

impl<R> AsyncBufRead for StderrReader<R>
where
    R: AsyncBytesRead + Unpin,
{
    fn poll_fill_buf(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<std::io::Result<&[u8]>> {
        let me = self.project();
        loop {
            match me.state {
                State::Sending(value) => {
                    ready!(me.sender.poll_reserve(cx))
                        .map_err(|err| io::Error::new(io::ErrorKind::BrokenPipe, err))?;
                    me.sender
                        .send_item(*value)
                        .map_err(|err| io::Error::new(io::ErrorKind::BrokenPipe, err))?;
                    *me.state = State::Reading(TryReadBytesLimited::new(1..=*value));
                }
                State::Reading(read) => {
                    if let Some(buf) = ready!(read.poll_reader(cx, Pin::new(me.inner)))? {
                        *me.state = State::Buffer(buf);
                    } else {
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            "got EOF reading with STDERR_READ",
                        )));
                    }
                }
                State::Buffer(bytes) if bytes.is_empty() => {
                    *me.state = State::Sending(DEFAULT_BUF_SIZE);
                }
                State::Buffer(bytes) => {
                    return Poll::Ready(Ok(&bytes[..]));
                }
            }
        }
    }

    fn consume(self: std::pin::Pin<&mut Self>, amt: usize) {
        let me = self.project();
        match me.state {
            State::Buffer(bytes) => bytes.advance(amt),
            _ => panic!("No buffer available"),
        }
    }
}
