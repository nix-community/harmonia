//! NAR listing: parse a NAR archive into a [`FileTree<NarFileInfo>`].

use std::collections::BTreeMap;
use std::io;

use serde::{Deserialize, Serialize};

use harmonia_file_core::{Directory, FileSystemObject, FileTree, Regular, Symlink};
use harmonia_utils_io::AsyncBytesRead;

use crate::archive::{NarMetaEvent, walk_nar_metadata};

/// Metadata for a file entry within a NAR archive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NarFileInfo {
    pub size: u64,
    #[serde(rename = "narOffset", default, skip_serializing_if = "Option::is_none")]
    pub nar_offset: Option<u64>,
}

/// Parse a NAR archive and produce a recursive file tree listing.
///
/// The resulting tree has the same JSON format as `nix nar ls --json --recursive`.
pub async fn parse_nar_listing<R>(reader: R) -> io::Result<FileTree<NarFileInfo>>
where
    R: AsyncBytesRead + Unpin,
{
    // Stack of (directory_name, entries) for directories being built.
    // Entries arrive strictly sorted, so collecting into a BTreeMap on
    // EndDirectory avoids per-entry tree traversal and node splits.
    let mut stack: Vec<DirFrame> = Vec::new();
    let mut result: Option<FileTree<NarFileInfo>> = None;

    walk_nar_metadata(reader, |event| {
        match event {
            NarMetaEvent::File {
                name,
                executable,
                size,
                nar_offset,
            } => {
                let node = FileTree(FileSystemObject::Regular(Regular {
                    executable,
                    contents: NarFileInfo {
                        size,
                        nar_offset: Some(nar_offset),
                    },
                }));
                insert_node(&mut stack, &mut result, name, node)?;
            }
            NarMetaEvent::Symlink { name, target } => {
                let node = FileTree(FileSystemObject::Symlink(Symlink {
                    target: target_str(target)?,
                }));
                insert_node(&mut stack, &mut result, name, node)?;
            }
            NarMetaEvent::StartDirectory { name } => {
                let dir_name = if stack.is_empty() {
                    None
                } else {
                    Some(name_str(name)?)
                };
                stack.push((dir_name, Vec::new()));
            }
            NarMetaEvent::EndDirectory => {
                let (name, entries) = stack
                    .pop()
                    .ok_or_else(|| io::Error::other("EndDirectory without StartDirectory"))?;
                let entries: BTreeMap<String, Box<FileTree<NarFileInfo>>> =
                    entries.into_iter().collect();
                let node = FileTree(FileSystemObject::Directory(Directory { entries }));
                if let Some((_, parent_entries)) = stack.last_mut() {
                    let key = name.ok_or_else(|| {
                        io::Error::new(io::ErrorKind::InvalidData, "missing directory entry name")
                    })?;
                    parent_entries.push((key, Box::new(node)));
                } else {
                    result = Some(node);
                }
            }
        }
        Ok(())
    })
    .await?;

    result.ok_or_else(|| io::Error::other("empty NAR"))
}

type DirFrame = (Option<String>, Vec<(String, Box<FileTree<NarFileInfo>>)>);

fn insert_node(
    stack: &mut [DirFrame],
    result: &mut Option<FileTree<NarFileInfo>>,
    name: &[u8],
    node: FileTree<NarFileInfo>,
) -> io::Result<()> {
    if let Some((_, entries)) = stack.last_mut() {
        entries.push((name_str(name)?, Box::new(node)));
    } else {
        *result = Some(node);
    }
    Ok(())
}

fn name_str(name: &[u8]) -> io::Result<String> {
    std::str::from_utf8(name)
        .map(str::to_owned)
        .map_err(|e| io::Error::other(format!("non-UTF-8 entry name: {e}")))
}

fn target_str(target: &[u8]) -> io::Result<String> {
    std::str::from_utf8(target)
        .map(str::to_owned)
        .map_err(|e| io::Error::other(format!("non-UTF-8 symlink target: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use futures_util::TryStreamExt;
    use harmonia_utils_io::BytesReader;

    use crate::archive::test_data;

    #[tokio::test]
    async fn test_listing_from_nar() {
        // Create a temp dir, dump it as NAR bytes, then parse the listing
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path();
        std::fs::write(dir.join("hello"), b"world").unwrap();
        std::fs::create_dir(dir.join("subdir")).unwrap();
        std::fs::write(dir.join("subdir/nested"), b"content").unwrap();
        std::os::unix::fs::symlink("hello", dir.join("link")).unwrap();

        // NarByteStream produces raw NAR bytes
        let byte_stream = crate::archive::NarByteStream::new(dir.to_owned());
        let chunks: Vec<bytes::Bytes> = byte_stream.try_collect().await.unwrap();
        let nar_bytes: Vec<u8> = chunks.into_iter().flatten().collect();

        // Parse the listing
        let reader = BytesReader::new(std::io::Cursor::new(nar_bytes.clone()));
        let listing = parse_nar_listing(reader).await.unwrap();

        // Verify structure
        let json = serde_json::to_value(&listing).unwrap();
        let root = json.as_object().unwrap();
        assert_eq!(root["type"], "directory");
        let entries = root["entries"].as_object().unwrap();
        assert!(entries.contains_key("hello"));
        assert!(entries.contains_key("subdir"));
        assert!(entries.contains_key("link"));
        assert_eq!(entries["hello"]["type"], "regular");
        assert_eq!(entries["hello"]["size"], 5);
        assert_eq!(entries["link"]["type"], "symlink");
        assert_eq!(entries["link"]["target"], "hello");
        assert_eq!(entries["subdir"]["type"], "directory");
        let sub_entries = entries["subdir"]["entries"].as_object().unwrap();
        assert!(sub_entries.contains_key("nested"));
        assert_eq!(sub_entries["nested"]["size"], 7);

        // Every narOffset must address the file's contents in the NAR itself.
        assert_contents_at(&nar_bytes, &entries["hello"], b"world");
        assert_contents_at(&nar_bytes, &sub_entries["nested"], b"content");

        // Only regular files carry an offset.
        assert!(entries["link"].get("narOffset").is_none());
        assert!(entries["subdir"].get("narOffset").is_none());
    }

    #[tokio::test]
    async fn test_listing_root_file_offset() {
        for (events, expected, contents) in [
            (test_data::text_file(), 96, b"Hello world!".as_slice()),
            (test_data::exec_file(), 128, b"Very cool stuff".as_slice()),
        ] {
            let nar_bytes = crate::archive::write_nar(events.iter());

            let reader = BytesReader::new(std::io::Cursor::new(nar_bytes.clone()));
            let listing = parse_nar_listing(reader).await.unwrap();

            let json = serde_json::to_value(&listing).unwrap();
            assert_eq!(json["narOffset"], expected);
            assert_contents_at(&nar_bytes, &json, contents);
        }
    }

    /// Assert that `entry`'s `narOffset` addresses `contents` within `nar`.
    fn assert_contents_at(nar: &[u8], entry: &serde_json::Value, contents: &[u8]) {
        let offset = entry["narOffset"].as_u64().unwrap() as usize;
        assert_eq!(&nar[offset..offset + contents.len()], contents);
    }

    #[tokio::test]
    async fn test_listing_chunked_stream_parity() {
        let nar_bytes = crate::archive::write_nar(test_data::dir_example().iter());
        let expected = parse_nar_listing(BytesReader::new(std::io::Cursor::new(nar_bytes.clone())))
            .await
            .unwrap();

        for chunk_size in [1usize, 3, 7, 13, 64] {
            let chunks: Vec<io::Result<Bytes>> = nar_bytes
                .chunks(chunk_size)
                .map(|c| Ok(Bytes::copy_from_slice(c)))
                .collect();
            let reader =
                harmonia_utils_io::BytesStreamReader::new(futures_util::stream::iter(chunks));
            let actual = parse_nar_listing(reader).await.unwrap();
            assert_eq!(actual, expected, "mismatch at chunk_size={chunk_size}");
        }
    }

    #[tokio::test]
    async fn test_listing_rejects_nonzero_padding_streaming_and_buffered() {
        let mut nar_bytes = crate::archive::write_nar(test_data::text_file().iter()).to_vec();
        let listing = parse_nar_listing(std::io::Cursor::new(Bytes::from(nar_bytes.clone())))
            .await
            .unwrap();
        let FileSystemObject::Regular(reg) = listing.0 else {
            panic!("expected root regular file");
        };
        let pad_offset = (reg.contents.nar_offset.unwrap() + reg.contents.size) as usize;
        nar_bytes[pad_offset] = 1;

        // Buffered fast path
        let err = parse_nar_listing(std::io::Cursor::new(Bytes::from(nar_bytes.clone())))
            .await
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);

        // Streaming slow path (splits across content and padding)
        let chunks: Vec<io::Result<Bytes>> = nar_bytes
            .chunks(5)
            .map(|c| Ok(Bytes::copy_from_slice(c)))
            .collect();
        let reader = harmonia_utils_io::BytesStreamReader::new(futures_util::stream::iter(chunks));
        let err = parse_nar_listing(reader).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[tokio::test]
    async fn test_listing_rejects_truncated_and_unsorted_entries() {
        let nar_bytes = crate::archive::write_nar(test_data::dir_example().iter());
        let name_pos = nar_bytes
            .windows(b"more".len())
            .position(|w| w == b"more")
            .unwrap();
        let sym_nar = crate::archive::write_nar(test_data::symlink().iter());
        let target_pos = sym_nar
            .windows(b"../deep".len())
            .position(|w| w == b"../deep")
            .unwrap();

        // Truncating before the final closing token or mid-name must error.
        for cut in [0, 1, 16, name_pos + 2, nar_bytes.len() - 1] {
            let err = parse_nar_listing(std::io::Cursor::new(Bytes::copy_from_slice(
                &nar_bytes[..cut],
            )))
            .await
            .unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof, "cut={cut}");
        }

        // Truncating mid-symlink-target must error.
        let err = parse_nar_listing(std::io::Cursor::new(Bytes::copy_from_slice(
            &sym_nar[..target_pos + 4],
        )))
        .await
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);

        // Duplicate / unsorted entries must be rejected.
        let unsorted = [
            crate::archive::NarEvent::StartDirectory { name: Bytes::new() },
            crate::archive::NarEvent::File {
                name: Bytes::from_static(b"b"),
                executable: false,
                size: 0,
                reader: std::io::Cursor::new(Bytes::new()),
            },
            crate::archive::NarEvent::File {
                name: Bytes::from_static(b"a"),
                executable: false,
                size: 0,
                reader: std::io::Cursor::new(Bytes::new()),
            },
            crate::archive::NarEvent::EndDirectory,
        ];
        let bad_nar = crate::archive::write_nar(unsorted.iter());
        let err = parse_nar_listing(std::io::Cursor::new(bad_nar))
            .await
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    /// A file name the listing rejects fails it without waiting for the
    /// file's contents, so a sender that stalls after the header can't stall it.
    #[test]
    fn test_listing_rejects_name_before_contents() {
        use futures_util::{FutureExt as _, StreamExt as _};

        let events = [
            crate::archive::NarEvent::StartDirectory { name: Bytes::new() },
            crate::archive::NarEvent::File {
                name: Bytes::from_static(b"\xff"),
                executable: false,
                size: 8,
                reader: std::io::Cursor::new(Bytes::from_static(b"payload!")),
            },
            crate::archive::NarEvent::EndDirectory,
        ];
        let nar = crate::archive::write_nar(events.iter());
        let at = nar.windows(8).position(|w| w == b"payload!").unwrap();
        let header = futures_util::stream::iter([Ok(nar.slice(..at))])
            .chain(futures_util::stream::pending::<io::Result<Bytes>>());
        let reader = harmonia_utils_io::BytesStreamReader::new(header);
        let err = parse_nar_listing(reader)
            .now_or_never()
            .expect("the listing waited for the contents")
            .unwrap_err();
        assert!(err.to_string().contains("non-UTF-8 entry name"), "{err}");
    }
}
