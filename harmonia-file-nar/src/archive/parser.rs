use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, ready};

use bytes::Buf;
use futures_core::Stream;
use pin_project_lite::pin_project;
use tokio::io::AsyncRead;
use tracing::trace;

use crate::ByteString;
use crate::padded_reader::PaddedReader;
use harmonia_utils_io::{AsyncBufReadCompat, AsyncBytesRead, BytesReader, Lending, LentReader};

use super::NarEvent;
use super::read_nar::{Inner, InnerState, NodeType};

// Cap fully-buffered length fields so a hostile NAR cannot force unbounded
// allocation; both well above NAME_MAX/PATH_MAX.
const MAX_ENTRY_NAME_LEN: u64 = 4096;
const MAX_SYMLINK_TARGET_LEN: u64 = 64 * 1024;

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
                    if len > MAX_SYMLINK_TARGET_LEN {
                        return Poll::Ready(Some(Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "symlink target too long",
                        ))));
                    }
                    let (len, aligned) = (len as usize, aligned as usize);
                    while buf.len() < aligned {
                        buf = ready!(reader.as_mut().poll_force_fill_buf(cx))?;
                    }

                    let target = buf.split_to(len);
                    buf.advance(aligned - len);
                    reader.as_mut().consume(aligned);
                    this.state.bump_next();
                    let name = this.name.take().unwrap_or_default();
                    return Poll::Ready(Some(Ok(NarEvent::Symlink { name, target })));
                }
                InnerState::ReadEntryName(len, aligned) => {
                    if len > MAX_ENTRY_NAME_LEN {
                        return Poll::Ready(Some(Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "directory entry name too long",
                        ))));
                    }
                    let (len, aligned) = (len as usize, aligned as usize);
                    while buf.len() < aligned {
                        buf = ready!(reader.as_mut().poll_force_fill_buf(cx))?;
                        trace!(len = buf.len(), "Reading name");
                    }
                    let name_buf = buf.split_to(len);
                    trace!(len = buf.len(), ?name_buf, "Read name");
                    *this.name = Some(name_buf);
                    buf.advance(aligned - len);
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
    use futures_util::stream::TryStreamExt as _;
    parse_nar(source)
        .and_then(NarEvent::read_file)
        .try_collect()
        .await
}

#[cfg(test)]
mod unittests {
    use std::io::{self, Cursor};

    use rstest::rstest;
    use tokio::fs::File;

    use crate::archive::read_nar::{TOK_DIR, TOK_ENTRY, TOK_ROOT, TOK_SYM};
    use crate::archive::{test_data, write_nar};

    use super::*;

    #[tokio::test]
    #[rstest]
    #[case::dir_example("test-data/test-dir.nar", test_data::dir_example())]
    #[case::exec_file("test-data/test-exec.nar", test_data::exec_file())]
    #[case::text_file("test-data/test-text.nar", test_data::text_file())]
    async fn test_parse_nar(#[case] file: &str, #[case] expected: test_data::TestNarEvents) {
        let io = File::open(file).await.unwrap();
        let actual = read_nar(io).await.unwrap();
        assert_eq!(actual, expected);
    }

    /// Hostile length fields must yield InvalidData, not allocate or panic.
    #[tokio::test]
    #[rstest]
    #[case::symlink_huge(TOK_SYM, 1 << 30)]
    #[case::symlink_wrap(TOK_SYM, u64::MAX)]
    #[case::name_huge(&[TOK_DIR, TOK_ENTRY].concat(), 1 << 30)]
    #[case::name_wrap(&[TOK_DIR, TOK_ENTRY].concat(), u64::MAX)]
    async fn reject_oversized_length(#[case] prefix: &[u8], #[case] len: u64) {
        let mut nar = Vec::from(TOK_ROOT);
        nar.extend_from_slice(prefix);
        nar.extend_from_slice(&len.to_le_bytes());
        nar.extend_from_slice(&[0u8; 64]); // enough for one drive() iteration

        let err = read_nar(Cursor::new(nar)).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[tokio::test]
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
