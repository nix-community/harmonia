use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, ready};

use bytes::Buf;
use futures_core::Stream;
use pin_project_lite::pin_project;
use tokio::io::AsyncRead;
use tracing::trace;

use crate::ByteString;
use crate::padded_reader::{PaddedReader, check_padding};
use crate::wire::calc_aligned;
use harmonia_utils_io::{AsyncBufReadCompat, AsyncBytesRead, BytesReader, Lending, LentReader};

use super::NarEvent;
use super::read_nar::{Inner, InnerState, NodeType};

// Cap fully-buffered length fields so a hostile NAR cannot force unbounded
// allocation; both well above NAME_MAX/PATH_MAX.
const MAX_ENTRY_NAME_LEN: u64 = 4096;
const MAX_SYMLINK_TARGET_LEN: u64 = 64 * 1024;

pin_project! {
    /// Parses a NAR into a stream of [`NarEvent`]s.
    ///
    /// Each [`NarEvent::File`] carries the parser's own reader. Read it to the
    /// end or drop it before polling the parser again. The parser skips any
    /// contents left unread without copying them, and still checks their
    /// padding.
    pub struct NarParser<R> {
        #[pin]
        reader: Lending<R, PaddedReader<R>>,
        name: Option<ByteString>,
        // Last entry name per open directory; entries must be strictly
        // increasing like Nix requires, which also rules out duplicates.
        // Empty means no entry seen yet (valid names are never empty).
        // Stored back to back, starting at `prev_starts`; copied because
        // `Bytes` slices would pin a read buffer per open directory.
        prev_names: Vec<u8>,
        prev_starts: Vec<usize>,
        // Bytes consumed so far; see [`NarParser::position`].
        position: u64,
        state: Inner<false>,
    }
}

/// Reject names that Nix's NAR parser also refuses; unpacking them to disk
/// would allow path traversal.
fn validate_entry_name(name: &[u8]) -> io::Result<()> {
    if name.is_empty() || name == b"." || name == b".." || name.contains(&b'/') || name.contains(&0)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "NAR contains invalid file name",
        ));
    }
    Ok(())
}

impl<R> NarParser<R>
where
    R: AsyncBytesRead + Unpin,
{
    pub fn new(reader: R) -> Self {
        Self {
            reader: Lending::new(reader),
            position: 0,
            name: None,
            prev_names: Vec::new(),
            prev_starts: Vec::new(),
            state: Inner {
                level: 0,
                state: InnerState::Root(0),
            },
        }
    }

    /// The parser's byte offset in the NAR (`narOffset`)
    pub fn position(&self) -> u64 {
        self.position
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
            InnerState::ReadContents(NodeType::ExecutableFile | NodeType::File, size, _) => {
                // The file's reader has drained the contents and their padding.
                *this.position = this.position.saturating_add(calc_aligned(size));
                this.state.bump_next();
            }
            InnerState::ReadDir => {
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
            *this.position += cnt as u64;
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

                    check_padding(&buf[len..aligned])?;
                    let target = buf.split_to(len);
                    buf.advance(aligned - len);
                    reader.as_mut().consume(aligned);
                    *this.position += aligned as u64;
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
                    check_padding(&buf[len..aligned])?;
                    let name_buf = buf.split_to(len);
                    trace!(len = buf.len(), ?name_buf, "Read name");
                    validate_entry_name(&name_buf)?;
                    if let Some(&start) = this.prev_starts.last() {
                        if name_buf[..] <= this.prev_names[start..] {
                            return Poll::Ready(Some(Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                "NAR directory entries are not sorted or contain duplicates",
                            ))));
                        }
                        this.prev_names.truncate(start);
                        this.prev_names.extend_from_slice(&name_buf);
                    }
                    *this.name = Some(name_buf);
                    buf.advance(aligned - len);
                    reader.as_mut().consume(aligned);
                    *this.position += aligned as u64;
                    this.state.bump_next();
                }
                InnerState::ReadDir => {
                    let name = this.name.take().unwrap_or_default();
                    this.prev_starts.push(this.prev_names.len());
                    return Poll::Ready(Some(Ok(NarEvent::StartDirectory { name })));
                }
                InnerState::FinishReadEntry => {
                    if let Some(start) = this.prev_starts.pop() {
                        this.prev_names.truncate(start);
                    }
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

    use bytes::Bytes;

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

    /// Build a NAR of a directory containing one empty file per name.
    fn dir_nar(names: &[&'static [u8]]) -> Bytes {
        let mut events: test_data::TestNarEvents =
            vec![NarEvent::StartDirectory { name: Bytes::new() }];
        for name in names {
            events.push(NarEvent::File {
                name: Bytes::from_static(name),
                executable: false,
                size: 0,
                reader: Cursor::new(Bytes::new()),
            });
        }
        events.push(NarEvent::EndDirectory);
        write_nar(events.iter())
    }

    /// Hostile entry names must be rejected: they would otherwise allow
    /// path traversal when a NAR is restored to disk.
    #[tokio::test]
    #[rstest]
    #[case::dot_dot(b"..")]
    #[case::dot(b".")]
    #[case::slash(b"foo/bar")]
    #[case::nul(b"foo\x00bar")]
    #[case::empty(b"")]
    async fn reject_invalid_entry_name(#[case] name: &'static [u8]) {
        let err = read_nar(Cursor::new(dir_nar(&[name]))).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    /// Duplicate or unsorted entries must be rejected, matching Nix's parser.
    #[tokio::test]
    #[rstest]
    #[case::duplicate(b"foo", b"foo")]
    #[case::unsorted(b"foo", b"bar")]
    async fn reject_duplicate_or_unsorted_entries(
        #[case] first: &'static [u8],
        #[case] second: &'static [u8],
    ) {
        let err = read_nar(Cursor::new(dir_nar(&[first, second])))
            .await
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    /// Each directory's entries are only compared with each other. After a
    /// subdirectory closes, the parent's next entry is compared with the
    /// subdirectory's name.
    #[tokio::test]
    async fn sort_check_is_per_directory() {
        use NarEvent::EndDirectory as End;
        let dir = |name: &'static [u8]| NarEvent::StartDirectory {
            name: Bytes::from_static(name),
        };
        let file = |name: &'static [u8]| NarEvent::File {
            name: Bytes::from_static(name),
            executable: false,
            size: 0,
            reader: Cursor::new(Bytes::new()),
        };

        // "aa" is compared with "a", not with "a/z", and "aa/a" is the first
        // entry of its directory.
        let events = [
            dir(b""),
            dir(b"a"),
            file(b"z"),
            End,
            dir(b"aa"),
            file(b"a"),
            End,
            End,
        ];
        let actual = read_nar(Cursor::new(write_nar(events.iter()))).await;
        assert_eq!(actual.unwrap(), events);

        // After "b/z", the root's last entry is still "b".
        let events = [dir(b""), dir(b"b"), file(b"z"), End, file(b"a"), End];
        let err = read_nar(Cursor::new(write_nar(events.iter())))
            .await
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    /// Like Nix, padding after file contents and strings has to be zero.
    #[tokio::test]
    #[rstest]
    async fn reject_nonzero_padding(#[values(1, 3, 8, 64_000)] chunk_size: usize) {
        for (case, nar) in test_data::nonzero_padding() {
            let err = read_nar(test_data::Chunked::new(nar.clone(), chunk_size))
                .await
                .unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidData, "{case}");
            let reader = BytesReader::new(test_data::Chunked::new(nar, chunk_size));
            let err = crate::parse_nar_listing(reader).await.unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidData, "{case}");
        }
    }

    /// A NAR that ends inside a file's contents or their padding fails, even
    /// when the listing skips the contents.
    #[tokio::test]
    #[rstest]
    async fn reject_truncated_contents(
        #[values(1, 3, 8, 64_000)] chunk_size: usize,
        #[values(false, true)] in_padding: bool,
    ) {
        for (events, contents) in [
            (test_data::text_file(), b"Hello world!".as_slice()),
            (test_data::dir_example(), b"Very cool stuff".as_slice()),
        ] {
            let nar = write_nar(events.iter());
            let at = nar.windows(contents.len()).position(|w| w == contents);
            // In the padding, cut before its last byte.
            let end = if in_padding {
                contents.len().next_multiple_of(8) - 1
            } else {
                5
            };
            let nar = nar.slice(..at.unwrap() + end);
            let err = read_nar(test_data::Chunked::new(nar.clone(), chunk_size))
                .await
                .unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
            let reader = BytesReader::new(test_data::Chunked::new(nar, chunk_size));
            let err = crate::parse_nar_listing(reader).await.unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
        }
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

    #[tokio::test]
    async fn empty_file_does_not_wait() {
        use futures_util::{FutureExt as _, StreamExt as _, stream};
        use tokio::io::AsyncReadExt as _;
        use tokio_util::io::StreamReader;

        use crate::archive::read_nar::TOK_FILE;

        let head = Bytes::from([TOK_ROOT, TOK_FILE, &0u64.to_le_bytes()].concat());
        let source = stream::iter([Ok::<_, io::Error>(head)]).chain(stream::pending());
        let mut events = std::pin::pin!(parse_nar(StreamReader::new(source)));
        let Some(Ok(NarEvent::File {
            size: 0,
            mut reader,
            ..
        })) = events.next().await
        else {
            panic!("expected an empty file");
        };
        let read = reader.read_to_end(&mut Vec::new()).now_or_never();
        assert_eq!(read.expect("the reader waited").unwrap(), 0);
    }
}
