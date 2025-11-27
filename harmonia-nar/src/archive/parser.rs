use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, ready};

use bytes::Buf;
use futures::Stream;
use pin_project_lite::pin_project;
use tokio::io::AsyncRead;
use tracing::trace;

use crate::padded_reader::PaddedReader;
use harmonia_store_core::ByteString;
use harmonia_store_core::io::{
    AsyncBufReadCompat, AsyncBytesRead, BytesReader, Lending, LentReader,
};

use super::NarEvent;
use super::read_nar::{Inner, InnerState, NodeType};

pin_project! {
    pub struct NarParser<R> {
        #[pin]
        reader: Lending<R, PaddedReader<R>>,
        name: Option<ByteString>,
        parsed: usize,
        state: Inner<false>,
    }
}

impl<R> NarParser<R>
where
    R: AsyncBytesRead + Unpin,
{
    pub fn new(reader: R) -> Self {
        Self {
            reader: Lending::new(reader),
            parsed: 0,
            name: None,
            state: Inner {
                level: 0,
                state: InnerState::Root(0),
            },
        }
    }
}

type ParsedReader<R> = AsyncBufReadCompat<LentReader<PaddedReader<R>>>;
impl<R> Stream for NarParser<R>
where
    R: AsyncBytesRead + Unpin,
{
    type Item = io::Result<NarEvent<ParsedReader<R>>>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();
        let mut reader = ready!(this.reader.as_mut().poll_reader(cx))?;
        match this.state.state {
            InnerState::ReadContents(NodeType::ExecutableFile | NodeType::File, _, _)
            | InnerState::ReadDir => {
                this.state.bump_next();
            }
            InnerState::FinishReadEntry => {
                this.state.bump_next();
                if this.state.is_eof() {
                    return Poll::Ready(None);
                }
            }
            InnerState::Eof => return Poll::Ready(None),
            _ => {}
        }
        loop {
            let mut buf = ready!(reader.as_mut().poll_fill_buf(cx))?;
            if buf.is_empty() {
                return Poll::Ready(Some(Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "EOF while reading NAR",
                ))));
            }
            let cnt = this.state.drive(&buf)?;
            buf.advance(cnt);
            reader.as_mut().consume(cnt);
            trace!(state=?this.state.state, cnt, "Loop state");
            match this.state.state {
                InnerState::ReadContents(
                    node_type @ (NodeType::ExecutableFile | NodeType::File),
                    size,
                    _,
                ) => {
                    let reader = this.reader.lend(|r| PaddedReader::new(r, size));
                    let name = this.name.take().unwrap_or_default();
                    return Poll::Ready(Some(Ok(NarEvent::File {
                        name,
                        executable: node_type == NodeType::ExecutableFile,
                        size,
                        reader: AsyncBufReadCompat::new(reader),
                    })));
                }
                InnerState::ReadContents(NodeType::Symlink, len, aligned) => {
                    let aligned = aligned.try_into().map_err(|_| {
                        io::Error::new(io::ErrorKind::InvalidData, "Symlink target way too long")
                    })?;
                    while buf.len() < aligned {
                        buf = ready!(reader.as_mut().poll_force_fill_buf(cx))?;
                    }

                    let target = buf.split_to(len as usize);
                    buf.advance(aligned - len as usize);
                    reader.as_mut().consume(aligned);
                    this.state.bump_next();
                    let name = this.name.take().unwrap_or_default();
                    return Poll::Ready(Some(Ok(NarEvent::Symlink { name, target })));
                }
                InnerState::ReadEntryName(len, aligned) => {
                    let aligned = aligned.try_into().map_err(|_| {
                        io::Error::new(io::ErrorKind::InvalidData, "Entry name way too long")
                    })?;
                    while buf.len() < aligned {
                        buf = ready!(reader.as_mut().poll_force_fill_buf(cx))?;
                        trace!(len = buf.len(), "Reading name");
                    }
                    let name_buf = buf.split_to(len as usize);
                    trace!(len = buf.len(), ?name_buf, "Read name");
                    *this.name = Some(name_buf);
                    buf.advance(aligned - len as usize);
                    reader.as_mut().consume(aligned);
                    this.state.bump_next();
                }
                InnerState::ReadDir => {
                    let name = this.name.take().unwrap_or_default();
                    return Poll::Ready(Some(Ok(NarEvent::StartDirectory { name })));
                }
                InnerState::FinishReadEntry => {
                    return Poll::Ready(Some(Ok(NarEvent::EndDirectory)));
                }
                InnerState::Eof => return Poll::Ready(None),
                _ => {}
            }
        }
    }
}

pub fn parse_nar<R>(
    reader: R,
) -> impl Stream<Item = io::Result<NarEvent<ParsedReader<BytesReader<R>>>>>
where
    R: AsyncRead + Unpin,
{
    let reader = BytesReader::new(reader);
    NarParser::new(reader)
}

#[cfg(any(test, feature = "test"))]
pub async fn read_nar<R>(source: R) -> io::Result<crate::archive::test_data::TestNarEvents>
where
    R: AsyncRead + Unpin,
{
    use futures::stream::TryStreamExt as _;
    parse_nar(source)
        .and_then(NarEvent::read_file)
        .try_collect()
        .await
}

#[cfg(test)]
mod unittests {
    use std::io::Cursor;

    use bytes::Bytes;
    use futures::TryStreamExt;
    use pretty_assertions::assert_eq;
    use rstest::rstest;
    use tokio::fs::File;
    use tokio::io::AsyncReadExt as _;

    use crate::archive::{test_data, write_nar};

    use super::*;

    #[test_log::test(tokio::test)]
    #[rstest]
    #[case::dir_example("test-data/test-dir.nar", test_data::dir_example())]
    #[case::exec_file("test-data/test-exec.nar", test_data::exec_file())]
    #[case::text_file("test-data/test-text.nar", test_data::text_file())]
    async fn test_parse_nar(#[case] file: &str, #[case] expected: test_data::TestNarEvents) {
        let io = File::open(file).await.unwrap();
        let s = parse_nar(io)
            .and_then(|event| async {
                Ok(match event {
                    NarEvent::File {
                        name,
                        executable,
                        size,
                        mut reader,
                    } => {
                        let mut content = Vec::new();
                        reader.read_to_end(&mut content).await?;
                        NarEvent::File {
                            name,
                            executable,
                            size,
                            reader: Cursor::new(Bytes::from(content)),
                        }
                    }
                    NarEvent::Symlink { name, target } => NarEvent::Symlink { name, target },
                    NarEvent::StartDirectory { name } => NarEvent::StartDirectory { name },
                    NarEvent::EndDirectory => NarEvent::EndDirectory,
                })
            })
            .try_collect::<test_data::TestNarEvents>()
            .await
            .unwrap();
        assert_eq!(s, expected);
    }

    #[test_log::test(tokio::test)]
    #[rstest]
    #[case::text_file(test_data::text_file())]
    #[case::exec_file(test_data::exec_file())]
    #[case::empty_file(test_data::empty_file())]
    #[case::empty_file_in_dir(test_data::empty_file_in_dir())]
    #[case::symlink(test_data::symlink())]
    #[case::empty_dir(test_data::empty_dir())]
    #[case::empty_dir_in_dir(test_data::empty_dir_in_dir())]
    #[case::dir_example(test_data::dir_example())]
    async fn parse_written(#[case] events: test_data::TestNarEvents) {
        let contents = write_nar(events.iter());
        let actual = read_nar(Cursor::new(contents)).await.unwrap();
        assert_eq!(events, actual);
    }
}
