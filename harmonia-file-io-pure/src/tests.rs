use tokio::io::AsyncWriteExt;

use harmonia_file_core::*;

use crate::sink::{DirectorySink, FileSystemSink};
use crate::source::MemoryTreeSource;
use crate::*;

/// Build a test tree:
/// ```text
/// .
/// ├── bar/
/// │   ├── baz  (executable, 19 bytes)
/// │   └── quux -> /over/there
/// └── foo  (15 bytes)
/// ```
async fn sample_tree() -> MemoryTree {
    let mut builder = MemoryTreeBuilder::new();
    {
        let mut root = builder.sink().create_directory().await.unwrap();

        {
            let mut bar = root
                .create_child("bar")
                .await
                .unwrap()
                .create_directory()
                .await
                .unwrap();
            bar.create_child("baz")
                .await
                .unwrap()
                .create_regular_file(true)
                .await
                .unwrap()
                .write_all(&vec![0u8; 19])
                .await
                .unwrap();
            bar.create_child("quux")
                .await
                .unwrap()
                .create_symlink("/over/there")
                .await
                .unwrap();
        }

        root.create_child("foo")
            .await
            .unwrap()
            .create_regular_file(false)
            .await
            .unwrap()
            .write_all(&vec![0u8; 15])
            .await
            .unwrap();
    }
    builder.build()
}

#[tokio::test]
async fn source_lstat() {
    let tree = sample_tree().await;
    let src = MemoryTreeSource::new(&tree);

    let root = src.lstat().await.unwrap();
    assert_eq!(root.file_type, FileType::Directory);

    let foo = src.open("foo").await.unwrap();
    let foo_stat = foo.lstat().await.unwrap();
    assert_eq!(foo_stat.file_type, FileType::Regular);
    assert_eq!(foo_stat.file_size, Some(15));
    assert!(!foo_stat.executable);

    let baz = src.open("bar").await.unwrap().open("baz").await.unwrap();
    let baz_stat = baz.lstat().await.unwrap();
    assert_eq!(baz_stat.file_type, FileType::Regular);
    assert_eq!(baz_stat.file_size, Some(19));
    assert!(baz_stat.executable);

    let quux = src.open("bar").await.unwrap().open("quux").await.unwrap();
    let quux_stat = quux.lstat().await.unwrap();
    assert_eq!(quux_stat.file_type, FileType::Symlink);
}

#[tokio::test]
async fn source_read_ops() {
    use tokio::io::AsyncReadExt;

    let tree = sample_tree().await;
    let src = MemoryTreeSource::new(&tree);

    let mut data = Vec::new();
    src.open("foo")
        .await
        .unwrap()
        .read_file()
        .await
        .unwrap()
        .read_to_end(&mut data)
        .await
        .unwrap();
    assert_eq!(data.len(), 15);

    let target = src
        .open("bar")
        .await
        .unwrap()
        .open("quux")
        .await
        .unwrap()
        .read_link()
        .await
        .unwrap();
    assert_eq!(target, "/over/there");

    use futures_util::StreamExt;
    let entries: Vec<String> = src
        .entries()
        .await
        .unwrap()
        .map(|r| r.unwrap().0)
        .collect()
        .await;
    assert_eq!(entries, ["bar", "foo"]);
}

#[tokio::test]
async fn list_deep_matches_nix_json() {
    let tree = sample_tree().await;
    let src = MemoryTreeSource::new(&tree);
    let listing = list_deep(&src).await.unwrap();
    let json = serde_json::to_value(&listing).unwrap();

    let expected: serde_json::Value = serde_json::from_str(
        r#"{
          "type": "directory",
          "entries": {
            "bar": {
              "type": "directory",
              "entries": {
                "baz": {
                  "type": "regular",
                  "executable": true,
                  "size": 19
                },
                "quux": {
                  "type": "symlink",
                  "target": "/over/there"
                }
              }
            },
            "foo": {
              "type": "regular",
              "size": 15
            }
          }
        }"#,
    )
    .unwrap();

    assert_eq!(json, expected);
}

#[tokio::test]
async fn list_shallow_matches_nix_json() {
    let tree = sample_tree().await;
    let src = MemoryTreeSource::new(&tree);
    let listing = list_shallow(&src).await.unwrap();
    let json = serde_json::to_value(&listing).unwrap();

    let expected: serde_json::Value = serde_json::from_str(
        r#"{
          "type": "directory",
          "entries": {
            "bar": {},
            "foo": {}
          }
        }"#,
    )
    .unwrap();

    assert_eq!(json, expected);
}

#[tokio::test]
async fn deep_listing_roundtrip_serde() {
    let tree = sample_tree().await;
    let src = MemoryTreeSource::new(&tree);
    let listing = list_deep(&src).await.unwrap();
    let json = serde_json::to_string(&listing).unwrap();
    let parsed: FileTree<Stat> = serde_json::from_str(&json).unwrap();
    let json2 = serde_json::to_string(&parsed).unwrap();
    assert_eq!(json, json2);
}

#[tokio::test]
async fn shallow_listing_roundtrip_serde() {
    let tree = sample_tree().await;
    let src = MemoryTreeSource::new(&tree);
    let listing = list_shallow(&src).await.unwrap();
    let json = serde_json::to_string(&listing).unwrap();
    let parsed: ShallowTree<Stat> = serde_json::from_str(&json).unwrap();
    let json2 = serde_json::to_string(&parsed).unwrap();
    assert_eq!(json, json2);
}

#[tokio::test]
async fn source_exists() {
    let tree = sample_tree().await;
    let src = MemoryTreeSource::new(&tree);
    assert!(src.exists("bar").await);
    assert!(src.exists("foo").await);
    assert!(!src.exists("nonexistent").await);

    let bar = src.open("bar").await.unwrap();
    assert!(bar.exists("baz").await);
    assert!(!bar.exists("nope").await);
}
