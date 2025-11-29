/*

## Root
- "nix-archive-1" :: String
- Node

## Node
- "(" :: String
- (FileNode | SymlinkNode | DirectoryNode)
- ")" :: String

## FileNode
- "type" => "regular" :: String
- ("executable" => "" :: String)?
- "contents" => Bytes

## SymlinkNode
- "type" => "symlink" :: String
- "target" => String

## DirectoryNode
- "type" => "directory" :: String
- ("entry" => DirEntry)*

### DirEntry

- "(" :: String
- "name" => String
- "node" => Node
- ")" :: String

Root
b"\x0d\0\0\0\0\0\0\0nix-archive-1\0\0\0\1\0\0\0\0\0\0\0(\0\0\0\0\0\0\0\4\0\0\0\0\0\0\0type\0\0\0\0"
b"\7\0\0\0\0\0\0\0regular\0\x0a\0\0\0\0\0\0\0executable\0\0\0\0\0\0\0\0\0\0\0\0\0\0\8\0\0\0\0\0\0\0contents"
b"\7\0\0\0\0\0\0\0regular\0\8\0\0\0\0\0\0\0contents"
b"\7\0\0\0\0\0\0\0symlink\0\6\0\0\0\0\0\0\0target\0\0"
b"\9\0\0\0\0\0\0\0directory\0\0\0\0\0\0\0"
b"\4\0\0\0\0\0\0\0node\0\0\0\0\1\0\0\0\0\0\0\0(\0\0\0\0\0\0\0\4\0\0\0\0\0\0\0type\0\0\0\0"
b"\1\0\0\0\0\0\0\0)\0\0\0\0\0\0\0"
b"\1\0\0\0\0\0\0\0)\0\0\0\0\0\0\0\1\0\0\0\0\0\0\0)\0\0\0\0\0\0\0"

Root
"nix-archive-1" "(" "type" => SelectNode

SelectNode
"directory" => ReadDir
"regular" "executable" "" "contents" => ReadContentsLen
"regular" "contents" => ReadContentsLen
"symlink" "target" => ReadContentsLen

ReadContentsLen
u64 => ReadContents

ReadContents
bytes => FinishNode

FinishNode
")" => level > 0 ? FinishEntry : EOF

ReadDir
push level
=> ReadEntries

FinishEntry
")" => ReadEntries

ReadEntries
"entry" "(" "name" => ReadEntryNameLen
")" => FinishReadEntry

FinishReadEntry
level > 1 ? pop level & FinishEntry : EOF

ReadEntryNameLen
len => ReadEntryName

ReadEntryName
bytes => ReadNode

ReadNode
"node" "(" "type" => SelectNode


EOF
 */

use std::cmp::min;
use std::io;
use std::ops::Range;
use std::pin::Pin;
use std::task::{Context, Poll, ready};

use bytes::Bytes;
use pin_project_lite::pin_project;
use tokio::io::{AsyncBufRead, AsyncRead};
use tracing::{error, trace};

use super::radix_tree::{RLookup, RMatch, RTree};
use crate::wire::{ZEROS, calc_aligned};
use harmonia_utils_io::{AsyncBytesRead, DrainInto};

// https://github.com/rust-lang/rust/issues/131415
#[allow(unsafe_code)]
const fn copy_from_slice(dst: &mut [u8], src: &[u8]) {
    if dst.len() != src.len() {
        panic!("failed");
    }
    // SAFETY: `self` is valid for `self.len()` elements by definition, and `src` was
    // checked to have the same length. The slices cannot overlap because
    // mutable references are exclusive.
    unsafe {
        std::ptr::copy_nonoverlapping(src.as_ptr(), dst.as_mut_ptr(), dst.len());
    }
}

const fn encode<const R: usize>(s: &[u8]) -> [u8; R] {
    let mut ret = [0u8; R];
    let (len, data) = ret.split_at_mut(size_of::<u64>());
    copy_from_slice(len, &(s.len() as u64).to_le_bytes());
    let (data, _padding) = data.split_at_mut(s.len());
    copy_from_slice(data, s);
    ret
}

const fn get_slice_mut(src: &mut [u8], range: Range<usize>) -> &mut [u8] {
    let (_prefix, rest) = src.split_at_mut(range.start);
    let (ret, _postfix) = rest.split_at_mut(range.end - range.start);
    ret
}

pub(crate) const fn concat<const R: usize>(list: &[&[u8]]) -> [u8; R] {
    let mut ret = [0u8; R];
    let mut idx = 0;
    let mut pos = 0;
    while idx < list.len() {
        let src = list[idx];
        let dst = get_slice_mut(&mut ret, pos..(pos + src.len()));
        copy_from_slice(dst, src);
        pos += src.len();
        idx += 1;
    }
    ret
}

macro_rules! encoding {
    ($value:expr) => {{
        const N: &[u8] = $value;
        const TN: usize = calc_aligned(N.len() as u64) as usize + size_of::<u64>();
        const E: &[u8] = &encode::<{ TN }>(N);
        E
    }};
}
macro_rules! concat_slice {
    ($($e:expr),+ $(,)?) => {{
        const LEN : usize = $($e.len() + )+ 0;
        let src = [$($e),+];
        &crate::archive::read_nar::concat::<{LEN}>(&src)
    }};
}
macro_rules! token {
    ($($e:literal),+ $(,)?) => {
        concat_slice!($(encoding!($e)),+)
    };
}
pub(crate) use concat_slice;
pub(crate) use token;

pub const TOK_ROOT: &[u8] = token!(b"nix-archive-1", b"(", b"type");
pub const TOK_PAR: &[u8] = encoding!(b")");
pub const TOK_ENTRY: &[u8] = token!(b"entry", b"(", b"name");
pub const TOK_NODE: &[u8] = token!(b"node", b"(", b"type");
pub const TOK_FILE_E: &[u8] = token!(b"regular", b"executable", b"", b"contents");
pub const TOK_FILE: &[u8] = token!(b"regular", b"contents");
pub const TOK_SYM: &[u8] = token!(b"symlink", b"target");
pub const TOK_DIR: &[u8] = token!(b"directory");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NodeType {
    File,
    ExecutableFile,
    Symlink,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum InnerState {
    Root(u8),
    SelectNode(RLookup),
    ReadContentsLen(NodeType, [u8; 8], u8),
    ReadContents(NodeType, u64, u64),
    FinishNode(u8),
    ReadDir,
    FinishEntry(u8),
    ReadEntries(RLookup),
    FinishReadEntry,
    ReadEntryNameLen([u8; 8], u8),
    ReadEntryName(u64, u64),
    ReadNode(u8),
    Eof,
}

#[derive(Debug)]
pub(crate) struct Inner<const P: bool> {
    pub(crate) state: InnerState,
    pub(crate) level: usize,
}

macro_rules! read_token {
    ($state:ident, $token:ident, $self:ident, $read:ident, $parsed:ident, $buf:ident) => {
        let rem = &$token[$read as usize..];
        let cmp = min(rem.len(), $buf.len());
        if rem[..cmp] != $buf[..cmp] {
            error!(rem=?rem[..cmp], buf=?$buf[..cmp], "Invalid data");
            return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid data in NAR"));
        }
        $parsed += cmp;
        if cmp < rem.len() {
            $self.state = InnerState::$state($read + cmp as u8);
            break;
        }
        $buf = &$buf[cmp..];
    };
}

impl<const P: bool> Inner<P> {
    pub(crate) fn is_eof(&self) -> bool {
        matches!(&self.state, InnerState::Eof)
    }
    pub(crate) fn bump_next(&mut self) {
        assert!(!P, "Can only bump when not parsing content");
        self.bump_state();
    }
    fn bump_state(&mut self) {
        match self.state {
            InnerState::ReadContents(_, _, _) => self.state = InnerState::FinishNode(0),
            InnerState::ReadEntryName(_, _) => self.state = InnerState::ReadNode(0),
            InnerState::ReadDir => self.state = InnerState::ReadEntries(Default::default()),
            InnerState::FinishReadEntry => {
                if self.level > 1 {
                    self.level -= 1;
                    self.state = InnerState::FinishEntry(0);
                } else {
                    self.state = InnerState::Eof;
                }
            }
            _ => panic!("Invalid NAR state for bump"),
        }
    }
    pub(crate) fn drive(&mut self, mut buf: &[u8]) -> io::Result<usize> {
        let mut parsed = 0;
        loop {
            match self.state {
                /*
                "nix-archive-1" "(" "type" => SelectNode
                 */
                InnerState::Root(read) => {
                    trace!(self.level, read, parsed, "InnerState::Root");
                    read_token!(Root, TOK_ROOT, self, read, parsed, buf);
                    self.state = InnerState::SelectNode(Default::default());
                }

                /*
                "directory" => ReadDir
                "regular" "executable" "" "contents" => ReadContentsLen
                "regular" "contents" => ReadContentsLen
                "symlink" "target" => ReadContentsLen
                */
                InnerState::SelectNode(mut state) => {
                    trace!(self.level, ?state, parsed, "InnerState::SelectNode");
                    const NODE_SELECT: RTree<7, 6, 2, InnerState> = {
                        let mut tree = RTree::new();
                        tree.insert(TOK_DIR, InnerState::ReadDir);
                        tree.insert(
                            TOK_SYM,
                            InnerState::ReadContentsLen(NodeType::Symlink, ZEROS, 0),
                        );
                        tree.insert(
                            TOK_FILE,
                            InnerState::ReadContentsLen(NodeType::File, ZEROS, 0),
                        );
                        tree.insert(
                            TOK_FILE_E,
                            InnerState::ReadContentsLen(NodeType::ExecutableFile, ZEROS, 0),
                        );
                        tree
                    };

                    match NODE_SELECT.lookup(&mut state, buf) {
                        RMatch::Matched(read) => {
                            parsed += read;
                            buf = &buf[read..];
                            self.state = NODE_SELECT.get_value(&state);
                        }
                        RMatch::NeedsMore => {
                            parsed += buf.len();
                            self.state = InnerState::SelectNode(state);
                            break;
                        }
                        RMatch::Mismatch(_) => {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                "could not select node type",
                            ));
                        }
                    }
                }

                /*
                u64 => ReadContents
                */
                InnerState::ReadContentsLen(node_type, mut value, read) => {
                    let rem = min(buf.len(), 8 - read as usize);
                    let new_read = read + rem as u8;
                    value[read as usize..new_read as usize].copy_from_slice(&buf[..rem]);
                    parsed += rem;
                    if new_read < 8 {
                        trace!(
                            self.level,
                            read,
                            new_read,
                            rem,
                            parsed,
                            buf_len = buf.len(),
                            "InnerState::ReadContentsLen break"
                        );
                        self.state = InnerState::ReadContentsLen(node_type, value, new_read);
                        break;
                    } else {
                        trace!(
                            self.level,
                            read,
                            new_read,
                            rem,
                            parsed,
                            buf_len = buf.len(),
                            "InnerState::ReadContentsLen next"
                        );
                        buf = &buf[rem..];
                        let len = u64::from_le_bytes(value);
                        self.state = InnerState::ReadContents(node_type, len, calc_aligned(len));
                    }
                }

                /*
                bytes => FinishNode
                */
                InnerState::ReadContents(node_type, size, rem) => {
                    trace!(self.level, rem, parsed, "InnerState::ReadContents");
                    if !P {
                        break;
                    }
                    if (buf.len() as u64) < rem {
                        parsed += buf.len();
                        self.state =
                            InnerState::ReadContents(node_type, size, rem - buf.len() as u64);
                        break;
                    } else {
                        parsed += rem as usize;
                        buf = &buf[rem as usize..];
                        self.bump_state();
                    }
                }

                /*
                ")" => level > 0 ? FinishEntry : EOF
                */
                InnerState::FinishNode(read) => {
                    trace!(self.level, read, parsed, "InnerState::FinishNode");
                    read_token!(FinishNode, TOK_PAR, self, read, parsed, buf);
                    if self.level > 0 {
                        self.state = InnerState::FinishEntry(0);
                    } else {
                        self.state = InnerState::Eof;
                    }
                }

                /*
                push level
                => ReadEntries
                */
                InnerState::ReadDir => {
                    trace!(self.level, parsed, "InnerState::ReadDir");
                    self.level += 1;
                    if !P {
                        break;
                    }
                    self.bump_state();
                }

                /*
                ")" => ReadEntries
                */
                InnerState::FinishEntry(read) => {
                    trace!(self.level, read, parsed, "InnerState::FinishEntry");
                    read_token!(FinishEntry, TOK_PAR, self, read, parsed, buf);
                    self.state = InnerState::ReadEntries(Default::default());
                }

                /*
                "entry" "(" "name" => ReadEntryNameLen
                ")" => FinishReadEntry
                */
                InnerState::ReadEntries(mut state) => {
                    trace!(
                        self.level,
                        ?state,
                        parsed,
                        buf = buf.len(),
                        "InnerState::ReadEntries"
                    );
                    const ENTRY_SELECT: RTree<3, 2, 2, InnerState> = {
                        let mut tree = RTree::new();
                        tree.insert(TOK_ENTRY, InnerState::ReadEntryNameLen(ZEROS, 0));
                        tree.insert(TOK_PAR, InnerState::FinishReadEntry);
                        tree
                    };
                    match ENTRY_SELECT.lookup(&mut state, buf) {
                        RMatch::Matched(read) => {
                            parsed += read;
                            buf = &buf[read..];
                            self.state = ENTRY_SELECT.get_value(&state);
                            trace!(read, ?self.state, "MATCHED!");
                        }
                        RMatch::NeedsMore => {
                            trace!("NEEDS MORE");
                            parsed += buf.len();
                            self.state = InnerState::ReadEntries(state);
                            break;
                        }
                        RMatch::Mismatch(_) => {
                            error!("MISMATCH!");
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                "could not read entry",
                            ));
                        }
                    }
                }

                /*
                level > 1 ? pop level & FinishEntry : EOF
                */
                InnerState::FinishReadEntry => {
                    trace!(self.level, parsed, "InnerState::FinishReadEntry");
                    if !P {
                        break;
                    }
                    self.bump_state();
                }

                /*
                len => ReadEntryName
                */
                InnerState::ReadEntryNameLen(mut value, read) => {
                    trace!(self.level, read, parsed, "InnerState::ReadEntryNameLen");
                    let rem = min(buf.len(), 8 - read as usize);
                    let new_read = read + rem as u8;
                    value[read as usize..new_read as usize].copy_from_slice(&buf[..rem]);
                    parsed += rem;
                    if new_read < 8 {
                        self.state = InnerState::ReadEntryNameLen(value, new_read);
                        break;
                    } else {
                        buf = &buf[rem..];
                        let len = u64::from_le_bytes(value);
                        self.state = InnerState::ReadEntryName(len, calc_aligned(len));
                    }
                }

                /*
                bytes => ReadNode
                */
                InnerState::ReadEntryName(len, rem) => {
                    trace!(self.level, len, rem, parsed, "InnerState::ReadEntryName");
                    if !P {
                        break;
                    }
                    if (buf.len() as u64) < rem {
                        parsed += buf.len();
                        self.state = InnerState::ReadEntryName(len, rem - buf.len() as u64);
                        break;
                    } else {
                        parsed += rem as usize;
                        buf = &buf[rem as usize..];
                        self.bump_state();
                    }
                }

                /*
                "node" "(" "type" => SelectNode
                */
                InnerState::ReadNode(read) => {
                    trace!(self.level, read, parsed, "InnerState::ReadNode");
                    read_token!(ReadNode, TOK_NODE, self, read, parsed, buf);
                    self.state = InnerState::SelectNode(Default::default());
                }
                InnerState::Eof => {
                    trace!(self.level, parsed, buf = buf.len(), "InnerState::EOF");
                    break;
                }
            }
        }
        Ok(parsed)
    }
}

pin_project! {
    #[derive(Debug)]
    pub struct NarReader<R> {
        #[pin]
        reader: R,
        parsed: usize,
        state: Inner<true>,
    }
}

impl<R> NarReader<R>
where
    R: AsyncBufRead,
{
    pub fn new(reader: R) -> NarReader<R> {
        NarReader {
            reader,
            parsed: 0,
            state: Inner {
                level: 0,
                state: InnerState::Root(0),
            },
        }
    }
}

impl<R> AsyncBufRead for NarReader<R>
where
    R: AsyncBufRead,
{
    fn poll_fill_buf(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<&[u8]>> {
        let this = self.project();
        trace!(parsed = *this.parsed, "poll_fill_buf reader");
        if *this.parsed == 0 && this.state.is_eof() {
            return Poll::Ready(Ok(b""));
        }
        let buf = match ready!(this.reader.poll_fill_buf(cx)) {
            Ok(buf) => buf,
            Err(err) => {
                error!(parsed = *this.parsed, ?err, "poll_fill_buf reader Error");
                return Err(err).into();
            }
        };
        trace!(
            parsed = *this.parsed,
            buf.len = buf.len(),
            "poll_fill_buf len"
        );
        if buf.len() > *this.parsed {
            *this.parsed += this.state.drive(&buf[*this.parsed..])?;
        }
        if buf.is_empty() && !this.state.is_eof() {
            error!(
                parsed = *this.parsed,
                len = buf[..*this.parsed].len(),
                "poll_fill_buf Error"
            );
            Poll::Ready(Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "not a complete NAR",
            )))
        } else {
            trace!(
                parsed = *this.parsed,
                len = buf[..*this.parsed].len(),
                "poll_fill_buf"
            );
            Poll::Ready(Ok(&buf[..*this.parsed]))
        }
    }

    fn consume(self: Pin<&mut Self>, amt: usize) {
        let this = self.project();
        assert!(
            *this.parsed >= amt,
            "consuming {} when parsed is {}",
            amt,
            *this.parsed
        );
        trace!(amt, parsed = *this.parsed, "consuming");
        this.reader.consume(amt);
        *this.parsed -= amt;
    }
}

impl<R> AsyncRead for NarReader<R>
where
    R: AsyncBufRead,
{
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let parsed = self.parsed;
        let rem = ready!(self.as_mut().poll_fill_buf(cx))?;
        trace!(
            len = rem.len(),
            buf.remaining = buf.remaining(),
            parsed,
            "poll_read"
        );
        if !rem.is_empty() {
            let amt = min(rem.len(), buf.remaining());
            buf.put_slice(&rem[0..amt]);
            self.consume(amt);
        }
        Poll::Ready(Ok(()))
    }
}

pin_project! {
    #[derive(Debug)]
    pub struct NarBytesReader<R> {
        #[pin]
        reader: R,
        parsed: usize,
        state: Inner<true>,
    }
}

impl<R> NarBytesReader<R>
where
    R: AsyncBytesRead,
{
    pub fn new(reader: R) -> NarBytesReader<R> {
        Self {
            reader,
            parsed: 0,
            state: Inner {
                level: 0,
                state: InnerState::Root(0),
            },
        }
    }
}

impl<R> AsyncBytesRead for NarBytesReader<R>
where
    R: AsyncBytesRead,
{
    fn poll_fill_buf(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<Bytes>> {
        let this = self.project();
        trace!(parsed = *this.parsed, "poll_fill_buf reader");
        if *this.parsed == 0 && this.state.is_eof() {
            return Poll::Ready(Ok(Bytes::new()));
        }
        let mut buf = match ready!(this.reader.poll_fill_buf(cx)) {
            Ok(buf) => buf,
            Err(err) => {
                error!(parsed = *this.parsed, ?err, "poll_fill_buf reader Error");
                return Err(err).into();
            }
        };
        trace!(
            parsed = *this.parsed,
            buf.len = buf.len(),
            "poll_fill_buf len"
        );
        if buf.len() > *this.parsed {
            *this.parsed += this.state.drive(&buf[*this.parsed..])?;
        }
        if buf.is_empty() && !this.state.is_eof() {
            error!(
                parsed = *this.parsed,
                len = buf[..*this.parsed].len(),
                "poll_fill_buf Error"
            );
            Poll::Ready(Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "not a complete NAR",
            )))
        } else {
            trace!(
                parsed = *this.parsed,
                len = buf[..*this.parsed].len(),
                "poll_fill_buf"
            );
            buf.truncate(*this.parsed);
            Poll::Ready(Ok(buf))
        }
    }

    fn poll_force_fill_buf(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<Bytes>> {
        let this = self.project();
        trace!(parsed = *this.parsed, "poll_force_fill_buf reader");
        if *this.parsed == 0 && this.state.is_eof() {
            return Poll::Ready(Ok(Bytes::new()));
        }
        let mut buf = match ready!(this.reader.poll_force_fill_buf(cx)) {
            Ok(buf) => buf,
            Err(err) => {
                error!(
                    parsed = *this.parsed,
                    ?err,
                    "poll_force_fill_buf reader Error"
                );
                return Err(err).into();
            }
        };
        trace!(
            parsed = *this.parsed,
            buf.len = buf.len(),
            "poll_force_fill_buf len"
        );
        if buf.len() > *this.parsed {
            *this.parsed += this.state.drive(&buf[*this.parsed..])?;
        }
        if buf.is_empty() && !this.state.is_eof() {
            error!(
                parsed = *this.parsed,
                len = buf[..*this.parsed].len(),
                "poll_force_fill_buf Error"
            );
            Poll::Ready(Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "not a complete NAR",
            )))
        } else {
            trace!(
                parsed = *this.parsed,
                len = buf[..*this.parsed].len(),
                "poll_force_fill_buf"
            );
            buf.truncate(*this.parsed);
            Poll::Ready(Ok(buf))
        }
    }

    fn consume(self: Pin<&mut Self>, amt: usize) {
        let this = self.project();
        assert!(
            *this.parsed >= amt,
            "consuming {} when parsed is {}",
            amt,
            *this.parsed
        );
        trace!(amt, parsed = *this.parsed, "consuming");
        this.reader.consume(amt);
        *this.parsed -= amt;
    }

    fn prepare(self: Pin<&mut Self>, additional: usize) {
        self.project().reader.prepare(additional);
    }
}

impl<R> AsyncRead for NarBytesReader<R>
where
    R: AsyncBytesRead,
{
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let parsed = self.parsed;
        let rem = ready!(self.as_mut().poll_fill_buf(cx))?;
        trace!(
            len = rem.len(),
            buf.remaining = buf.remaining(),
            parsed,
            "poll_read"
        );
        if !rem.is_empty() {
            let amt = min(rem.len(), buf.remaining());
            buf.put_slice(&rem[0..amt]);
            self.consume(amt);
        }
        Poll::Ready(Ok(()))
    }
}

impl<R> DrainInto<R> for NarBytesReader<R>
where
    R: AsyncBytesRead,
{
    fn poll_drain(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        loop {
            let len = ready!(self.as_mut().poll_fill_buf(cx))?.len();
            if len == 0 {
                break;
            }
            self.as_mut().consume(len);
        }
        Poll::Ready(Ok(()))
    }

    fn into_inner(self) -> R {
        self.reader
    }
}

#[cfg(test)]
mod unittests {
    use std::io;
    use std::time::Duration;

    use bytes::{BufMut as _, Bytes, BytesMut};
    use rstest::rstest;
    use tokio::io::{AsyncReadExt, BufReader};
    use tokio_test::io::Builder;
    use tracing::trace;

    use crate::archive::test_data::*;
    use crate::archive::write_nar;

    use super::NarReader;

    #[test_log::test(tokio::test)]
    #[rstest]
    #[case::text_file(text_file())]
    #[case::exec_file(exec_file())]
    #[case::empty_file(empty_file())]
    #[case::empty_file_in_dir(empty_file_in_dir())]
    #[case::empty_dir(empty_dir())]
    #[case::empty_dir_in_dir(empty_dir_in_dir())]
    #[case::symlink(symlink())]
    #[case::dir_example(dir_example())]
    async fn read_nar(
        #[case] events: TestNarEvents,
        #[values(
            Ok(&b""[..]),
            Ok(&b"more"[..]),
            Err(io::ErrorKind::StorageFull)
        )]
        postfix: Result<&[u8], io::ErrorKind>,
        #[values(1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 51, 64_000)] chunk_size: usize,
    ) {
        let content = write_nar(&events);

        let mut buf = BytesMut::new();
        buf.put_slice(b"before");
        buf.put_slice(&content);
        if let Ok(postfix) = postfix {
            buf.put_slice(postfix);
        }
        let read_content = buf.freeze();

        let mock = {
            let mut b = Builder::new();
            for c in read_content.chunks(chunk_size) {
                b.read(c);
                b.wait(Duration::from_secs(0));
            }
            if let Err(err) = postfix {
                b.wait(Duration::from_secs(0));
                b.read_error(io::Error::new(err, "unexpected read"));
            }
            b.build()
        };
        let mut buf_read = BufReader::new(mock);

        let mut prefix = [0u8; 6];
        buf_read.read_exact(&mut prefix).await.unwrap();
        {
            trace!(contents = content.len(), "Read NAR");
            let mut reader = NarReader::new(&mut buf_read);
            let mut actual = Vec::new();
            reader.read_to_end(&mut actual).await.unwrap();
            let actual = Bytes::from(actual);
            trace!(actual = actual.len(), "Read NAR Done");
            assert_eq!(actual, content);
        }

        let mut rest = Vec::new();
        let res = buf_read.read_to_end(&mut rest).await;
        match (postfix, res) {
            (Ok(value), Ok(_)) => {
                assert_eq!(rest, value);
            }
            (Err(kind), Err(err)) => {
                assert_eq!(kind, err.kind());
            }
            (_, Err(err)) => {
                panic!("Unexpected read failure {err:?}");
            }
            (Err(kind), _) => {
                panic!("Read should fail with {kind:?} error");
            }
        }
    }
}

#[cfg(test)]
mod proptests {
    use std::time::{Duration, Instant};

    use bytes::{BufMut as _, Bytes, BytesMut};
    use proptest::prelude::{TestCaseError, any};
    use proptest::proptest;
    use tokio::io::{AsyncReadExt as _, BufReader};
    use tokio_test::io::Builder;
    use tracing::{info, trace};

    use crate::archive::NarReader;
    use crate::test::arbitrary::archive::arb_nar_contents;

    #[test_log::test]
    fn proptest_read_nar() {
        let r = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        proptest!(|(
            content in arb_nar_contents(20, 20, 5),
            chunk_size in any::<proptest::sample::Index>(),
        )| {
            let now = Instant::now();
            r.block_on(async {
                let mut buf = BytesMut::new();
                buf.put_slice(b"before");
                buf.put_slice(&content);
                buf.put_slice(b"more");
                let read_content = buf.freeze();

                let mut b = Builder::new();
                let chunk_size = chunk_size.index(read_content.len()) + 1;
                for c in read_content.chunks(chunk_size) {
                    b.read(c);
                    b.wait(Duration::from_secs(0));
                }
                let mock = b.build();
                let mut buf_read = BufReader::new(mock);

                let mut prefix = [0u8; 6];
                buf_read.read_exact(&mut prefix).await.unwrap();
                trace!(contents=content.len(), "Read NAR");
                let mut reader = NarReader::new(&mut buf_read);
                let mut actual = Vec::new();
                reader.read_to_end(&mut actual).await.unwrap();
                let actual = Bytes::from(actual);
                trace!(actual=actual.len(), "Read NAR Done");
                assert_eq!(actual, content);

                let mut rest = Vec::new();
                buf_read.read_to_end(&mut rest).await.unwrap();
                assert_eq!(rest, b"more");

                Ok(()) as Result<(), TestCaseError>
            })?;
            info!("Completed test {}", now.elapsed().as_secs_f64());
        })
    }
}
