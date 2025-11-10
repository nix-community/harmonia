use std::io;
use std::task::{Poll, ready};

use pin_project_lite::pin_project;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncRead};
use tracing::trace;

use crate::io::TryReadU64;

#[derive(Debug, Default)]
enum FramedReadState {
    ReadLen(TryReadU64),
    ReadData(u64),
    #[default]
    Eof,
}

pin_project! {
    #[derive(Debug)]
    pub struct FramedReader<R> {
        #[pin]
        reader: R,
        state: FramedReadState,
    }
}

impl<R> FramedReader<R>
where
    R: AsyncBufRead + Unpin,
{
    pub fn new(reader: R) -> Self {
        Self {
            reader,
            state: FramedReadState::ReadLen(TryReadU64::new()),
        }
    }
    pub async fn drain_all(&mut self) -> io::Result<u64> {
        let mut drained = 0;
        loop {
            trace!(drained, "FramedReader:drain_all: Reading");
            let amt = self.fill_buf().await?.len();
            trace!(drained, amt, "FramedReader:drain_all: Read");
            if amt == 0 {
                return Ok(drained);
            }
            drained += amt as u64;
            self.consume(amt);
        }
    }
}

impl<R> AsyncRead for FramedReader<R>
where
    R: AsyncBufRead,
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

impl<R> AsyncBufRead for FramedReader<R>
where
    R: AsyncBufRead,
{
    fn poll_fill_buf(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<io::Result<&[u8]>> {
        let mut me = self.project();
        loop {
            match me.state {
                FramedReadState::ReadLen(r) => {
                    trace!("FramedReader:poll_fill_buf: ReadLen");
                    let len = ready!(r.poll_reader(cx, me.reader.as_mut()))?.ok_or_else(|| {
                        io::Error::new(io::ErrorKind::UnexpectedEof, "EOF in framed reader")
                    })?;
                    trace!("FramedReader:poll_fill_buf: read len {}", len);
                    if len > 0 {
                        *me.state = FramedReadState::ReadData(len);
                    } else {
                        *me.state = FramedReadState::Eof;
                    }
                }
                FramedReadState::ReadData(remaining) => {
                    trace!("FramedReader:poll_fill_buf: ReadData {}", *remaining);
                    let buf = ready!(me.reader.poll_fill_buf(cx))?;
                    trace!("FramedReader:poll_fill_buf: Got {}", buf.len());
                    if buf.is_empty() {
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            "EOF in framed reader",
                        )));
                    }
                    let amt = std::cmp::min(buf.len(), *remaining as usize);
                    return Poll::Ready(Ok(&buf[..amt]));
                }
                FramedReadState::Eof => {
                    return Poll::Ready(Ok(&[]));
                }
            }
        }
    }

    fn consume(self: std::pin::Pin<&mut Self>, amt: usize) {
        let me = self.project();
        match me.state {
            FramedReadState::ReadData(remaining) => {
                trace!("FramedReader:consume: Buf {} {}", remaining, amt);
                *remaining -= amt as u64;
                if *remaining == 0 {
                    *me.state = FramedReadState::ReadLen(TryReadU64::new());
                }
                me.reader.consume(amt);
            }
            _ => panic!("Consume called in invalid state {amt}"),
        }
    }
}

#[cfg(test)]
mod unittests {
    use hex_literal::hex;
    use tokio::io::{AsyncReadExt as _, BufReader};
    use tokio_test::io::Builder;

    use super::*;

    #[tokio::test]
    async fn test_read_frames() {
        let mut mock = BufReader::with_capacity(
            3,
            Builder::new()
                .read(&hex!(
                    "0100 0000 0000 0000 20 0400 0000 0000 0000 4142 4344"
                ))
                .read(&hex!("0100 0000 0000 0000 45 0000 0000 0000 0000 46"))
                .build(),
        );
        let mut reader = FramedReader::new(&mut mock);

        let mut s = String::new();
        reader.read_to_string(&mut s).await.unwrap();
        assert_eq!(s, " ABCDE");

        let mut buf = Vec::new();
        mock.read_to_end(&mut buf).await.unwrap();
        assert_eq!(hex!("46"), &buf[..]);
    }
}
