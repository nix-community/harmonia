use std::cmp::max;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, ready};

use bytes::{Buf, Bytes};
use pin_project_lite::pin_project;
use tokio::io::{AsyncRead, ReadBuf};

use harmonia_store_core::io::{AsyncBytesRead, DrainInto};
use harmonia_store_core::wire::ZEROS;

#[derive(Debug, Clone)]
enum State {
    Content(u64),
    ReadPadding(u8, u8),
    Padding(u8),
    Eof,
}

pin_project! {
    pub struct PaddedReader<R> {
        #[pin]
        reader: R,
        trailer_size: u8,
        padding: [u8; 8],
        state: State,
    }
}

impl<R> PaddedReader<R> {
    pub fn new(reader: R, len: u64) -> Self {
        let content = len & !7;
        let trailer_size = (len - content) as u8;
        Self {
            reader,
            trailer_size,
            padding: ZEROS,
            state: State::Content(content),
        }
    }
    fn remaining_usize(self: Pin<&mut Self>) -> (usize, usize, usize) {
        let this = self.project();
        let aligned = (*this.trailer_size + 7) & !7;
        match this.state {
            State::Content(rem) => (
                (*rem).try_into().unwrap_or(usize::MAX),
                (*rem + *this.trailer_size as u64)
                    .try_into()
                    .unwrap_or(usize::MAX),
                (*rem + aligned as u64).try_into().unwrap_or(usize::MAX),
            ),
            State::ReadPadding(start, _) => (
                0,
                (*this.trailer_size - *start) as usize,
                (aligned - *start) as usize,
            ),
            State::Padding(start) => (
                0,
                (*this.trailer_size - *start) as usize,
                (aligned - *start) as usize,
            ),
            State::Eof => (0, 0, 0),
        }
    }
}

impl<R> AsyncRead for PaddedReader<R>
where
    R: AsyncRead,
{
    #[allow(unsafe_code)]
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let mut this = self.project();
        loop {
            match &mut this.state {
                State::Content(rem) => {
                    let n = {
                        let mut buf = buf.take((*rem).try_into().unwrap_or(usize::MAX));
                        let ptr = buf.filled().as_ptr();
                        ready!(this.reader.as_mut().poll_read(cx, &mut buf)?);

                        // Ensure the pointer does not change from under us
                        assert_eq!(ptr, buf.filled().as_ptr());
                        buf.filled().len()
                    };
                    if n == 0 {
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            "EOF before end of content",
                        )));
                    }

                    // SAFETY: This is guaranteed to be the number of initialized (and read)
                    // bytes due to the invariants provided by `ReadBuf::filled`.
                    unsafe { buf.assume_init(n) };
                    buf.advance(n);

                    *rem -= n as u64;
                    if *rem == 0 {
                        if *this.trailer_size > 0 {
                            *this.state = State::ReadPadding(0, 0);
                        } else {
                            *this.state = State::Eof;
                        }
                    }
                }
                State::ReadPadding(start, end) => {
                    let mut tail_buf = ReadBuf::new(this.padding);
                    tail_buf.advance(*end as usize);
                    ready!(this.reader.as_mut().poll_read(cx, &mut tail_buf))?;
                    if tail_buf.filled().len() == *end as usize {
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            "EOF before end of padding",
                        )));
                    }
                    *end = tail_buf.filled().len() as u8;
                    if *end == 8 {
                        *this.state = State::Padding(*start);
                    }
                }
                State::Padding(pos) => {
                    let bound =
                        std::cmp::min((*this.trailer_size - *pos) as usize, buf.remaining());
                    buf.put_slice(&this.padding[*pos as usize..bound]);
                    *pos += bound as u8;
                    if *pos == *this.trailer_size {
                        *this.state = State::Eof;
                    }
                }
                State::Eof => break,
            }
        }
        Poll::Ready(Ok(()))
    }
}

impl<R> AsyncBytesRead for PaddedReader<R>
where
    R: AsyncBytesRead,
{
    fn poll_force_fill_buf(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<bytes::Bytes>> {
        let (content, rem, trailing) = self.as_mut().remaining_usize();
        let mut this = self.project();
        match this.state {
            State::Content(_) => {
                let mut buf = ready!(this.reader.as_mut().poll_force_fill_buf(cx))?;
                if buf.len() >= content {
                    while buf.len() < trailing {
                        buf = ready!(this.reader.as_mut().poll_force_fill_buf(cx))?;
                    }
                    Poll::Ready(Ok(buf.split_to(rem)))
                } else {
                    Poll::Ready(Ok(buf))
                }
            }
            State::ReadPadding(start, end) => {
                while *end < 8 {
                    let mut tail_buf = ReadBuf::new(this.padding);
                    tail_buf.advance(*end as usize);
                    ready!(this.reader.as_mut().poll_read(cx, &mut tail_buf))?;
                    if tail_buf.filled().len() == *end as usize {
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            "EOF before end of padding",
                        )));
                    }
                    *end = tail_buf.filled().len() as u8;
                }
                let start = *start;
                *this.state = State::Padding(start);
                let mut buf = Bytes::from_owner(*this.padding);
                buf.truncate(*this.trailer_size as usize);
                buf.advance(start as usize);
                Poll::Ready(Ok(buf))
            }
            _ => Poll::Ready(Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "EOF while force reading",
            ))),
        }
    }

    fn poll_fill_buf(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<bytes::Bytes>> {
        let (content, rem, trailing) = self.as_mut().remaining_usize();
        let mut this = self.project();
        match this.state {
            State::Content(_) => {
                let mut buf = ready!(this.reader.as_mut().poll_fill_buf(cx))?;
                if buf.len() >= content {
                    while buf.len() < trailing {
                        buf = ready!(this.reader.as_mut().poll_force_fill_buf(cx))?;
                    }
                    Poll::Ready(Ok(buf.split_to(rem)))
                } else {
                    Poll::Ready(Ok(buf))
                }
            }
            State::ReadPadding(start, end) => {
                while *end < 8 {
                    let mut tail_buf = ReadBuf::new(this.padding);
                    tail_buf.advance(*end as usize);
                    ready!(this.reader.as_mut().poll_read(cx, &mut tail_buf))?;
                    if tail_buf.filled().len() == *end as usize {
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            "EOF before end of padding",
                        )));
                    }
                    *end = tail_buf.filled().len() as u8;
                }
                let start = *start;
                *this.state = State::Padding(start);
                let mut buf = Bytes::from_owner(*this.padding);
                buf.truncate(*this.trailer_size as usize);
                buf.advance(start as usize);
                Poll::Ready(Ok(buf))
            }
            State::Padding(start) => {
                let mut buf = Bytes::from_owner(*this.padding);
                buf.truncate(*this.trailer_size as usize);
                buf.advance(*start as usize);
                Poll::Ready(Ok(buf))
            }
            State::Eof => Poll::Ready(Ok(Bytes::new())),
        }
    }

    fn prepare(self: Pin<&mut Self>, additional: usize) {
        self.project().reader.prepare(additional)
    }

    fn consume(mut self: Pin<&mut Self>, amt: usize) {
        let (_, rem, aligned) = self.as_mut().remaining_usize();
        assert!(amt <= rem, "Consuming more than content");
        let mut this = self.project();
        if amt == rem {
            if matches!(this.state, State::Content(_) | State::ReadPadding(_, _)) {
                this.reader.as_mut().consume(aligned);
            }
            *this.state = State::Eof;
            return;
        }
        match this.state {
            State::Content(c_rem) => {
                this.reader.as_mut().consume(amt);
                let u_rem = (*c_rem).try_into().unwrap_or(usize::MAX);
                if amt > u_rem {
                    let start = (amt - u_rem) as u8;
                    *this.state = State::ReadPadding(start, start);
                } else {
                    *c_rem -= amt as u64;
                }
            }
            State::ReadPadding(start, end) => {
                this.reader.as_mut().consume(amt);
                *start += amt as u8;
                *end = max(*start, *end);
            }
            State::Padding(start) => {
                *start += amt as u8;
            }
            State::Eof => {}
        }
    }
}

impl<R> DrainInto<R> for PaddedReader<R>
where
    R: AsyncBytesRead,
{
    fn poll_drain(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        loop {
            let buf = ready!(self.as_mut().poll_fill_buf(cx))?;
            if buf.is_empty() {
                break;
            }
            self.as_mut().consume(buf.len());
        }
        Poll::Ready(Ok(()))
    }

    fn into_inner(self) -> R {
        self.reader
    }
}

#[cfg(test)]
mod unittests {
    use std::future::poll_fn;
    use std::io;
    use std::pin::pin;

    use bytes::Bytes;
    use rstest::rstest;

    use super::PaddedReader;
    use harmonia_store_core::io::AsyncBytesRead as _;

    #[tokio::test]
    #[rstest]
    #[case::part_content(0, 5, 17, b" world! From Vino\0\0After")]
    #[case::rest_of_content(5, 11, 6, b"m Vino\0\0After")]
    #[case::all_content(0, 16, 6, b"m Vino\0\0After")]
    #[case::all_content_some_padding(0, 19, 3, b"ino\0\0After")]
    #[case::all_content_all_padding(0, 22, 0, b"After")]
    #[case::some_content_some_padding(5, 14, 3, b"ino\0\0After")]
    #[case::some_padding(16, 3, 3, b"ino\0\0After")]
    #[case::all_padding(16, 6, 0, b"After")]
    #[case::rest_of_padding(19, 3, 0, b"After")]
    #[case::no_consume(22, 0, 0, b"After")]
    async fn test_consume(
        #[case] pre: usize,
        #[case] consume: usize,
        #[case] remaining: usize,
        #[case] left: &'static [u8],
    ) {
        let input = Bytes::from_static(b"Hello world! From Vino\0\0After");
        let mut reader = io::Cursor::new(input.clone());
        let padded_reader = PaddedReader::new(&mut reader, 22);
        {
            let mut padded = pin!(padded_reader);
            if pre > 0 {
                padded.as_mut().consume(pre);
            }
            padded.as_mut().consume(consume);
            let (_content, actual, _aligned) = padded.as_mut().remaining_usize();
            assert_eq!(actual, remaining);
        }
        let mut reader = pin!(reader);
        let expected = poll_fn(|cx| reader.as_mut().poll_fill_buf(cx))
            .await
            .unwrap();
        assert_eq!(Bytes::from_static(left), expected);
    }

    #[tokio::test]
    #[should_panic(expected = "Consuming more than content")]
    async fn test_consume_assert() {
        let input = Bytes::from_static(b"Hello world! From Vino\0\0After");
        let mut reader = io::Cursor::new(input.clone());
        let padded_reader = PaddedReader::new(&mut reader, 22);
        let mut padded = pin!(padded_reader);
        padded.as_mut().consume(23);
    }

    // Read to end
    // fill_buf
    // force_fill_buf
    // TryReadBytesLimited
    // Read line
}
