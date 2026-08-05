//! NAR dump: serialize a [`FileSystemSource`] to NAR wire format.

use std::io;

use futures_util::StreamExt;
use harmonia_file_io_pure::FileSystemSource;
use tokio::io::{AsyncWrite, AsyncWriteExt};

use super::read_nar::{
    TOK_DIR, TOK_ENTRY, TOK_FILE, TOK_FILE_E, TOK_NODE, TOK_PAR, TOK_ROOT, TOK_SYM,
};

/// Write a NAR archive from a [`FileSystemSource`] to an async writer.
pub async fn dump_source<S: FileSystemSource, W: AsyncWrite + Unpin>(
    source: &S,
    writer: &mut W,
) -> io::Result<()> {
    writer.write_all(TOK_ROOT).await?;
    dump_node(source, writer).await?;
    writer.write_all(TOK_PAR).await?;
    Ok(())
}

async fn dump_node<S: FileSystemSource, W: AsyncWrite + Unpin>(
    source: &S,
    w: &mut W,
) -> io::Result<()> {
    let stat = source.lstat().await.map_err(to_io)?;
    match stat.file_type {
        harmonia_file_io_pure::FileType::Regular => {
            w.write_all(if stat.executable {
                TOK_FILE_E
            } else {
                TOK_FILE
            })
            .await?;
            let size = stat.file_size.unwrap_or(0);
            w.write_all(&size.to_le_bytes()).await?;
            let mut reader = source.read_file().await.map_err(to_io)?;
            let copied = tokio::io::copy(&mut reader, w).await?;
            if copied != size {
                return Err(io::Error::other(format!(
                    "file size mismatch: expected {size}, wrote {copied}"
                )));
            }
            let padding = crate::wire::calc_padding(size);
            if padding > 0 {
                w.write_all(&crate::wire::ZEROS[..padding]).await?;
            }
        }
        harmonia_file_io_pure::FileType::Symlink => {
            w.write_all(TOK_SYM).await?;
            let target = source.read_link().await.map_err(to_io)?;
            write_str(w, target.as_bytes()).await?;
        }
        harmonia_file_io_pure::FileType::Directory => {
            w.write_all(TOK_DIR).await?;
            let mut entries = source.entries().await.map_err(to_io)?;
            let mut unhacked = Vec::<(String, S::Child)>::new();
            while let Some(item) = entries.next().await {
                let (name, child) = item.map_err(to_io)?;
                let name = if cfg!(target_os = "macos") {
                    strip_case_hack(&name).to_owned()
                } else {
                    name
                };
                unhacked.push((name, child));
            }
            // The restorer appends a case-hack suffix on case-insensitive
            // filesystems, so re-sort by the cleaned name to match a NAR
            // produced on a case-sensitive one.
            unhacked.sort_by(|a, b| a.0.cmp(&b.0));
            if let Some(dup) = unhacked.windows(2).find(|w| w[0].0 == w[1].0) {
                return Err(io::Error::other(format!(
                    "case-hack collision on entry name {:?}",
                    dup[0].0
                )));
            }
            for (name, child) in unhacked {
                w.write_all(TOK_ENTRY).await?;
                write_str(w, name.as_bytes()).await?;
                w.write_all(TOK_NODE).await?;
                Box::pin(dump_node(&child, w)).await?;
                w.write_all(TOK_PAR).await?; // close node
                w.write_all(TOK_PAR).await?; // close entry
            }
        }
    }
    Ok(())
}

/// Removes everything from the case-hack suffix onward, recovering the original
/// name the restorer disambiguated on a case-insensitive filesystem.
fn strip_case_hack(name: &str) -> &str {
    match name.find(crate::archive::CASE_HACK_SUFFIX) {
        Some(pos) => &name[..pos],
        None => name,
    }
}

/// Write a single wire-encoded string (length prefix + data + padding).
/// Only used for variable-length data (entry names, symlink targets).
async fn write_str<W: AsyncWrite + Unpin>(w: &mut W, s: &[u8]) -> io::Result<()> {
    w.write_all(&(s.len() as u64).to_le_bytes()).await?;
    w.write_all(s).await?;
    let padding = crate::wire::calc_padding(s.len() as u64);
    if padding > 0 {
        w.write_all(&crate::wire::ZEROS[..padding]).await?;
    }
    Ok(())
}

fn to_io(e: impl std::error::Error) -> io::Error {
    io::Error::other(e.to_string())
}

#[cfg(test)]
mod tests {
    use futures_util::StreamExt as _;
    use harmonia_file_io_pure::*;
    use harmonia_utils_io::BytesReader;

    use super::*;
    use crate::archive::parser::NarParser;
    use crate::archive::test_data;
    use crate::archive::write_nar;

    /// Build a MemoryTree from test events via NarWriter + restore_to_sink,
    /// then dump it with dump_source and parse back to events.
    async fn dump_and_collect(events: &test_data::TestNarEvents) -> test_data::TestNarEvents {
        // Events → NAR bytes
        let nar_bytes = write_nar(events.iter());

        // NAR bytes → MemoryTree
        let mut builder = MemoryTreeBuilder::new();
        let reader = BytesReader::new(std::io::Cursor::new(nar_bytes));
        crate::archive::restorer::restore_to_sink(reader, builder.sink())
            .await
            .unwrap();
        let tree = builder.build();

        // MemoryTree → NAR bytes (via dump_source)
        let src = MemoryTreeSource::new(&tree);
        let mut nar2 = Vec::new();
        dump_source(&src, &mut nar2).await.unwrap();

        // NAR bytes → events
        let reader = BytesReader::new(std::io::Cursor::new(nar2));
        let mut parser = NarParser::new(reader);
        let mut result = test_data::TestNarEvents::new();
        while let Some(event) = parser.next().await {
            result.push(event.unwrap().read_file().await.unwrap());
        }
        result
    }

    #[tokio::test]
    async fn test_dump_dir() {
        let result = dump_and_collect(&test_data::dir_example()).await;
        assert_eq!(result, test_data::dir_example());
    }

    #[tokio::test]
    async fn test_dump_text_file() {
        let result = dump_and_collect(&test_data::text_file()).await;
        assert_eq!(result, test_data::text_file());
    }

    #[tokio::test]
    async fn test_dump_exec_file() {
        let result = dump_and_collect(&test_data::exec_file()).await;
        assert_eq!(result, test_data::exec_file());
    }

    #[tokio::test]
    async fn test_dump_empty_file() {
        let result = dump_and_collect(&test_data::empty_file()).await;
        assert_eq!(result, test_data::empty_file());
    }

    #[tokio::test]
    async fn test_dump_symlink() {
        let result = dump_and_collect(&test_data::symlink()).await;
        assert_eq!(result, test_data::symlink());
    }
}
