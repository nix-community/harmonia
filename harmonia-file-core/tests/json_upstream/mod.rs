//! Tests that verify JSON serialization matches upstream Nix format
//!
//! Tests against data in:
//! - `src/libutil-tests/data/nar-listing/` — file tree listings (size, no contents)
//! - `src/libutil-tests/data/memory-source-accessor/` — file trees with contents

use harmonia_file_core::*;
use harmonia_utils_test::json_upstream::libutil_test_data_path;
use harmonia_utils_test::test_upstream_json;

// ---------------------------------------------------------------------------
// nar-listing: FileTree with size metadata (no contents)
// ---------------------------------------------------------------------------

/// Content type for NAR listings — just `size`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct ListingStat {
    size: u64,
}

fn deep_listing() -> FileTree<ListingStat> {
    FileTree(FileSystemObject::Directory(Directory {
        entries: {
            let mut m = std::collections::BTreeMap::new();
            m.insert(
                "bar".to_owned(),
                Box::new(FileTree(FileSystemObject::Directory(Directory {
                    entries: {
                        let mut d = std::collections::BTreeMap::new();
                        d.insert(
                            "baz".to_owned(),
                            Box::new(FileTree(FileSystemObject::Regular(Regular {
                                executable: true,
                                contents: ListingStat { size: 19 },
                            }))),
                        );
                        d.insert(
                            "quux".to_owned(),
                            Box::new(FileTree(FileSystemObject::Symlink(Symlink {
                                target: "/over/there".to_owned(),
                            }))),
                        );
                        d
                    },
                }))),
            );
            m.insert(
                "foo".to_owned(),
                Box::new(FileTree(FileSystemObject::Regular(Regular {
                    executable: false,
                    contents: ListingStat { size: 15 },
                }))),
            );
            m
        },
    }))
}

fn shallow_listing() -> ShallowTree<ListingStat> {
    FileSystemObject::Directory(Directory {
        entries: {
            let mut m = std::collections::BTreeMap::new();
            m.insert("bar".to_owned(), Opaque);
            m.insert("foo".to_owned(), Opaque);
            m
        },
    })
}

test_upstream_json!(
    nar_listing_deep,
    libutil_test_data_path("nar-listing/deep.json"),
    deep_listing()
);

test_upstream_json!(
    nar_listing_shallow,
    libutil_test_data_path("nar-listing/shallow.json"),
    shallow_listing()
);

// ---------------------------------------------------------------------------
// memory-source-accessor: FileTree with string contents
// ---------------------------------------------------------------------------

/// Content type for in-memory trees — file contents as a string.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct StringContents {
    contents: String,
}

fn simple_tree() -> FileTree<StringContents> {
    FileTree(FileSystemObject::Regular(Regular {
        executable: false,
        contents: StringContents {
            contents: "asdf".to_owned(),
        },
    }))
}

fn complex_tree() -> FileTree<StringContents> {
    FileTree(FileSystemObject::Directory(Directory {
        entries: {
            let mut m = std::collections::BTreeMap::new();
            m.insert(
                "bar".to_owned(),
                Box::new(FileTree(FileSystemObject::Directory(Directory {
                    entries: {
                        let mut d = std::collections::BTreeMap::new();
                        d.insert(
                            "baz".to_owned(),
                            Box::new(FileTree(FileSystemObject::Regular(Regular {
                                executable: true,
                                contents: StringContents {
                                    contents: "good day,\n\0\n\tworld!".to_owned(),
                                },
                            }))),
                        );
                        d.insert(
                            "quux".to_owned(),
                            Box::new(FileTree(FileSystemObject::Symlink(Symlink {
                                target: "/over/there".to_owned(),
                            }))),
                        );
                        d
                    },
                }))),
            );
            m.insert(
                "foo".to_owned(),
                Box::new(FileTree(FileSystemObject::Regular(Regular {
                    executable: false,
                    contents: StringContents {
                        contents: "hello\n\0\n\tworld!".to_owned(),
                    },
                }))),
            );
            m
        },
    }))
}

// memory-source-accessor JSON includes `"executable": false` for
// non-executable files. Our serde currently skips false values
// (matching `nix nar ls` behavior pre-NixOS/nix#15834).
// Once that PR lands, remove `skip_serializing_if` and use
// `test_upstream_json!` for full round-trip here too.
mod memory_source_accessor_simple {
    use super::*;
    #[test]
    fn from_json() {
        harmonia_utils_test::json_upstream::test_upstream_json_from_json(
            &libutil_test_data_path("memory-source-accessor/simple.json"),
            &simple_tree(),
        );
    }
}

mod memory_source_accessor_complex {
    use super::*;
    #[test]
    fn from_json() {
        harmonia_utils_test::json_upstream::test_upstream_json_from_json(
            &libutil_test_data_path("memory-source-accessor/complex.json"),
            &complex_tree(),
        );
    }
}
