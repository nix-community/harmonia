use crate::ByteString;

mod dumper;
mod parser;
pub(crate) mod radix_tree;
mod read_nar;
mod restorer;
pub mod test_data;
mod writer;

pub use dumper::{DumpOptions, DumpedFile, NarDumper, dump};
#[cfg(any(test, feature = "test"))]
pub use parser::read_nar;
pub use parser::{NarParser, parse_nar};
pub use read_nar::{NarBytesReader, NarReader};
pub use restorer::{NarRestorer, NarWriteError, RestoreOptions, restore};
pub use writer::NarWriter;
#[cfg(any(test, feature = "test"))]
pub use writer::write_nar;

pub const CASE_HACK_SUFFIX: &str = "~nix~case~hack~";

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Hash)]
pub enum NarEvent<R> {
    File {
        name: ByteString,
        executable: bool,
        size: u64,
        reader: R,
    },
    Symlink {
        name: ByteString,
        target: ByteString,
    },
    StartDirectory {
        name: ByteString,
    },
    EndDirectory,
}

impl<R> NarEvent<R>
where
    R: tokio::io::AsyncRead + Unpin,
{
    #[cfg(any(test, feature = "test"))]
    pub async fn read_file(self) -> std::io::Result<test_data::TestNarEvent> {
        use tokio::io::AsyncReadExt as _;
        match self {
            NarEvent::StartDirectory { name } => Ok(NarEvent::StartDirectory { name }),
            NarEvent::EndDirectory => Ok(NarEvent::EndDirectory),
            NarEvent::Symlink { name, target } => Ok(NarEvent::Symlink { name, target }),
            NarEvent::File {
                name,
                executable,
                size,
                mut reader,
            } => {
                let mut buf = Vec::new();
                reader.read_to_end(&mut buf).await?;
                let reader = std::io::Cursor::new(bytes::Bytes::from(buf));
                Ok(NarEvent::File {
                    name,
                    executable,
                    size,
                    reader,
                })
            }
        }
    }
}
