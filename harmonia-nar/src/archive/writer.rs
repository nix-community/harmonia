use std::{
    io::{self, Cursor},
    task::{Poll, ready},
};

use bytes::{Buf, BufMut, Bytes, BytesMut};
use futures::{FutureExt as _, Sink, SinkExt as _, StreamExt as _, stream::iter};
use pin_project_lite::pin_project;
use tokio::io::{AsyncBufRead, AsyncWrite};

use crate::{io::DEFAULT_RESERVED_BUF_SIZE, wire::calc_padding};

use super::{
    NarEvent,
    read_nar::{TOK_DIR, TOK_ENTRY, TOK_FILE, TOK_FILE_E, TOK_NODE, TOK_PAR, TOK_ROOT, TOK_SYM},
    test_data,
};

enum State {
    Ready,
    Flushing(u8),
    Eof,
}
impl State {
    fn is_flushing(&self) -> bool {
        matches!(self, State::Flushing(_))
    }
}

pin_project! {
    pub struct NarWriter<R, W> {
        state: State,
        buffer: BytesMut,
        cutoff: usize,
        #[pin]
        reader: Option<R>,
        #[pin]
        writer: W,
        level: usize,
    }
}

impl<R, W> NarWriter<R, W>
where
    W: AsyncWrite,
    R: AsyncBufRead,
{
    pub fn new(writer: W) -> Self {
        Self {
            state: State::Ready,
            buffer: BytesMut::new(),
            cutoff: DEFAULT_RESERVED_BUF_SIZE,
            reader: None,
            level: 0,
            writer,
        }
    }

    pub fn poll_flush_buf(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<(), io::Error>> {
        loop {
            ready!(self.as_mut().poll_ready(cx))?;
            if self.as_mut().project().buffer.is_empty() {
                break;
            }
            *self.as_mut().project().state = State::Flushing(0);
        }
        Poll::Ready(Ok(()))
    }
}

fn put_nix_slice(buf: &mut BytesMut, src: &[u8]) {
    buf.put_u64_le(src.len() as u64);
    buf.put_slice(src);
    buf.put_bytes(0, calc_padding(src.len() as u64));
}

impl<R, W> Sink<NarEvent<R>> for NarWriter<R, W>
where
    W: AsyncWrite,
    R: AsyncBufRead,
{
    type Error = io::Error;

    fn poll_ready(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        let mut this = self.project();
        if !this.state.is_flushing() && this.buffer.len() > *this.cutoff {
            *this.state = State::Flushing(0);
        }
        if let State::Flushing(cnt) = *this.state {
            while !this.buffer.is_empty() {
                let cnt = ready!(this.writer.as_mut().poll_write(cx, this.buffer))?;
                if cnt == 0 {
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "wrote zero bytes",
                    )));
                }
                this.buffer.advance(cnt);
            }
            if cnt > 0 {
                this.buffer.put_bytes(0, cnt as usize);
            }
            if *this.level == 0 {
                *this.state = State::Eof;
            } else {
                *this.state = State::Ready;
            }
        }
        if let Some(mut reader) = this.reader.as_mut().as_pin_mut() {
            loop {
                let buf = ready!(reader.as_mut().poll_fill_buf(cx))?;
                if !buf.is_empty() {
                    let amt = ready!(this.writer.as_mut().poll_write(cx, buf))?;
                    reader.as_mut().consume(amt);
                } else {
                    break;
                }
            }
            this.buffer.put_slice(TOK_PAR);
            if *this.level == 0 {
                *this.state = State::Eof;
            } else {
                this.buffer.put_slice(TOK_PAR);
            }
            this.reader.set(None);
        }
        Poll::Ready(Ok(()))
    }

    fn start_send(self: std::pin::Pin<&mut Self>, item: NarEvent<R>) -> Result<(), Self::Error> {
        let mut this = self.project();
        if *this.level == 0 {
            this.buffer.put_slice(TOK_ROOT);
        }
        match item {
            NarEvent::File {
                name,
                executable,
                size,
                reader,
            } => {
                if *this.level > 0 {
                    this.buffer.put_slice(TOK_ENTRY);
                    put_nix_slice(this.buffer, &name);
                    this.buffer.put_slice(TOK_NODE);
                }
                if executable {
                    this.buffer.put_slice(TOK_FILE_E);
                } else {
                    this.buffer.put_slice(TOK_FILE);
                }
                this.buffer.put_u64_le(size);
                *this.state = State::Flushing(calc_padding(size) as u8);
                this.reader.set(Some(reader));
            }
            NarEvent::Symlink { name, target } => {
                if *this.level > 0 {
                    this.buffer.put_slice(TOK_ENTRY);
                    put_nix_slice(this.buffer, &name);
                    this.buffer.put_slice(TOK_NODE);
                }
                this.buffer.put_slice(TOK_SYM);
                put_nix_slice(this.buffer, &target);
                this.buffer.put_slice(TOK_PAR);
                if *this.level == 0 {
                    *this.state = State::Eof;
                } else {
                    this.buffer.put_slice(TOK_PAR);
                }
            }
            NarEvent::StartDirectory { name } => {
                if *this.level > 0 {
                    this.buffer.put_slice(TOK_ENTRY);
                    put_nix_slice(this.buffer, &name);
                    this.buffer.put_slice(TOK_NODE);
                }
                this.buffer.put_slice(TOK_DIR);
                *this.level += 1;
            }
            NarEvent::EndDirectory => {
                this.buffer.put_slice(TOK_PAR);
                *this.level -= 1;
                if *this.level == 0 {
                    *this.state = State::Eof;
                } else {
                    this.buffer.put_slice(TOK_PAR);
                }
            }
        }
        Ok(())
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        ready!(self.as_mut().poll_flush_buf(cx))?;
        self.project().writer.poll_flush(cx)
    }

    fn poll_close(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        ready!(self.as_mut().poll_flush_buf(cx))?;
        self.project().writer.poll_shutdown(cx)
    }
}

pub fn write_nar<'e, E>(events: E) -> Bytes
where
    E: IntoIterator<Item = &'e test_data::TestNarEvent>,
{
    let mut buf = Vec::new();
    let mut writer = NarWriter::new(Cursor::new(&mut buf));
    let mut stream = iter(events).map(Clone::clone).map(Ok);
    writer
        .send_all(&mut stream)
        .now_or_never()
        .expect("BUG: NarWriter blocks")
        .expect("BUG: NarWriter returned error");
    writer
        .close()
        .now_or_never()
        .expect("BUG: NarWriter close blocks")
        .expect("BUG: NarWriter close errors");
    buf.into()
}

#[cfg(test)]
mod unittests {
    use futures::StreamExt as _;
    use futures::stream::iter;
    use rstest::rstest;
    use tempfile::tempdir;
    use tokio::fs::File;

    use crate::archive::{read_nar, test_data};
    use crate::io::BytesReader;

    use super::NarWriter;

    #[tokio::test]
    #[rstest]
    #[case::text_file(test_data::text_file(), test_data::text_file())]
    #[case::exec_file(test_data::exec_file(), test_data::exec_file())]
    #[case::empty_file(test_data::empty_file(), test_data::empty_file())]
    #[case::empty_file_in_dir(test_data::empty_file_in_dir(), test_data::empty_file_in_dir())]
    #[case::empty_dir(test_data::empty_dir(), test_data::empty_dir())]
    #[case::empty_dir_in_dir(test_data::empty_dir_in_dir(), test_data::empty_dir_in_dir())]
    #[case::symlink(test_data::symlink(), test_data::symlink())]
    #[case::dir_example(test_data::dir_example(), test_data::dir_example())]
    async fn write_test_data(
        #[case] events: test_data::TestNarEvents,
        #[case] expected: test_data::TestNarEvents,
    ) {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test-text.nar");

        let io = File::create(&path).await.unwrap();
        let encoder = NarWriter::new(io);
        let stream = iter(events).map(Ok);
        stream.forward(encoder).await.unwrap();

        let io = File::open(path).await.unwrap();
        let s = read_nar(BytesReader::new(io)).await.unwrap();
        pretty_assertions::assert_eq!(s, expected);
    }
}
