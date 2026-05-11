//! NAR listing: parse a NAR archive into a [`FileTree<NarFileInfo>`].

use std::collections::BTreeMap;

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};

use harmonia_file_core::{Directory, FileSystemObject, FileTree, Regular, Symlink};
use harmonia_utils_io::AsyncBytesRead;

use crate::archive::{NarEvent, NarParser};

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
pub async fn parse_nar_listing<R>(reader: R) -> std::io::Result<FileTree<NarFileInfo>>
where
    R: AsyncBytesRead + Unpin,
{
    let mut parser = NarParser::new(reader);

    // Stack of (name, entries) for directories being built.
    let mut stack: Vec<DirFrame> = Vec::new();
    let mut result: Option<FileTree<NarFileInfo>> = None;

    while let Some(event) = parser.next().await {
        let event = event?;
        match event {
            NarEvent::File {
                name,
                executable,
                size,
                mut reader,
            } => {
                // Drain the file contents (we only want metadata)
                tokio::io::copy(&mut reader, &mut tokio::io::sink()).await?;

                let info = NarFileInfo {
                    size,
                    nar_offset: None, // TODO: track byte offset
                };
                let node = FileTree(FileSystemObject::Regular(Regular {
                    executable,
                    contents: info,
                }));
                insert_node(&mut stack, &mut result, name_str(&name)?, node);
            }
            NarEvent::Symlink { name, target } => {
                let node = FileTree(FileSystemObject::Symlink(Symlink {
                    target: target_str(&target)?,
                }));
                insert_node(&mut stack, &mut result, name_str(&name)?, node);
            }
            NarEvent::StartDirectory { name } => {
                stack.push((Some(name_str(&name)?), BTreeMap::new()));
            }
            NarEvent::EndDirectory => {
                let (name, entries) = stack
                    .pop()
                    .ok_or_else(|| std::io::Error::other("EndDirectory without StartDirectory"))?;
                let node = FileTree(FileSystemObject::Directory(Directory { entries }));
                if let Some(name) = name {
                    insert_node(&mut stack, &mut result, name, node);
                } else {
                    result = Some(node);
                }
            }
        }
    }

    result.ok_or_else(|| std::io::Error::other("empty NAR"))
}

type DirFrame = (Option<String>, BTreeMap<String, Box<FileTree<NarFileInfo>>>);

#[allow(clippy::ptr_arg)]
fn insert_node(
    stack: &mut Vec<DirFrame>,
    result: &mut Option<FileTree<NarFileInfo>>,
    name: String,
    node: FileTree<NarFileInfo>,
) {
    if let Some((_, entries)) = stack.last_mut() {
        entries.insert(name, Box::new(node));
    } else {
        // Root-level non-directory
        *result = Some(node);
    }
}

fn name_str(name: &bytes::Bytes) -> std::io::Result<String> {
    std::str::from_utf8(name)
        .map(|s| s.to_owned())
        .map_err(|e| std::io::Error::other(format!("non-UTF-8 entry name: {e}")))
}

fn target_str(target: &bytes::Bytes) -> std::io::Result<String> {
    std::str::from_utf8(target)
        .map(|s| s.to_owned())
        .map_err(|e| std::io::Error::other(format!("non-UTF-8 symlink target: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::TryStreamExt;
    use harmonia_utils_io::BytesReader;

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
        let reader = BytesReader::new(std::io::Cursor::new(nar_bytes));
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
    }
}
