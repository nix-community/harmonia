use tokio::io::AsyncRead;

use harmonia_file_core::{Directory, FileSystemObject, FileTree, MemoryTree, Regular};

// ---------------------------------------------------------------------------
// Stat / FileType
// ---------------------------------------------------------------------------

/// The type of a file-system entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    Regular,
    Directory,
    Symlink,
}

/// Metadata for a file-system entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stat {
    pub file_type: FileType,
    pub file_size: Option<u64>,
    pub executable: bool,
}

// ---------------------------------------------------------------------------
// FileSystemSource trait
// ---------------------------------------------------------------------------

/// Read-side interface for accessing a file tree.
///
/// The accessor represents a node in the tree. Use [`open`](Self::open)
/// to navigate to children, or [`entries`](Self::entries) to iterate them.
// TODO: desugar `async fn` in trait to explicit associated type to give downstream code more
// expressive power at some point.
#[allow(async_fn_in_trait)]
pub trait FileSystemSource: Sized {
    type Error: std::error::Error + 'static;
    type Reader: AsyncRead + Unpin;
    type Child: FileSystemSource<Error = Self::Error>;
    type ChildThunk: std::future::Future<Output = Result<Self::Child, Self::Error>> + Unpin;
    type Entries<'a>: futures_core::Stream<Item = Result<(String, Self::ChildThunk), Self::Error>>
        + Unpin
    where
        Self: 'a;

    /// Get metadata for this node (does not follow symlinks).
    async fn lstat(&self) -> Result<Stat, Self::Error>;

    /// Read the contents of this node (must be a regular file).
    async fn read_file(&self) -> Result<Self::Reader, Self::Error>;

    /// Read the symlink target of this node (must be a symlink).
    async fn read_link(&self) -> Result<String, Self::Error>;

    /// Iterate over children of this directory node.
    ///
    /// Each item is a `(name, child)` pair where the child is already
    /// opened as a [`Self::Child`]. Returns an error if this node is not
    /// a directory. Individual entries may also fail (e.g. permission
    /// errors on a filesystem source).
    ///
    /// Entries are yielded in sorted order.
    async fn entries(&self) -> Result<Self::Entries<'_>, Self::Error>;

    /// Open a child entry by name.
    async fn open(&self, name: &str) -> Result<Self::Child, Self::Error>;

    /// Check whether a child exists.
    async fn exists(&self, name: &str) -> bool {
        self.open(name).await.is_ok()
    }
}

// ---------------------------------------------------------------------------
// MemoryTree implementation
// ---------------------------------------------------------------------------

/// A reference into a [`MemoryTree`] acting as a [`FileSystemSource`].
#[derive(Debug, Clone)]
pub struct MemoryTreeSource<'a> {
    node: &'a MemoryTree,
}

impl<'a> MemoryTreeSource<'a> {
    pub fn new(tree: &'a MemoryTree) -> Self {
        Self { node: tree }
    }
}

/// Errors from in-memory tree access.
#[derive(Debug, thiserror::Error)]
pub enum MemorySourceError {
    #[error("entry not found: {0}")]
    NotFound(String),
    #[error("not a regular file")]
    NotAFile,
    #[error("not a symlink")]
    NotASymlink,
    #[error("not a directory")]
    NotADirectory,
}

impl<'a> FileSystemSource for MemoryTreeSource<'a> {
    type Error = MemorySourceError;
    type Reader = std::io::Cursor<Vec<u8>>;
    type Child = Self;
    type ChildThunk = std::future::Ready<Result<Self, MemorySourceError>>;
    type Entries<'b>
        = MemoryTreeEntries<'a>
    where
        Self: 'b;

    async fn lstat(&self) -> Result<Stat, Self::Error> {
        Ok(stat_of(self.node))
    }

    async fn read_file(&self) -> Result<Self::Reader, Self::Error> {
        match self.node {
            FileTree(FileSystemObject::Regular(r)) => Ok(std::io::Cursor::new(r.contents.clone())),
            _ => Err(MemorySourceError::NotAFile),
        }
    }

    async fn read_link(&self) -> Result<String, Self::Error> {
        match self.node {
            FileTree(FileSystemObject::Symlink(s)) => Ok(s.target.clone()),
            _ => Err(MemorySourceError::NotASymlink),
        }
    }

    async fn entries(&self) -> Result<Self::Entries<'_>, Self::Error> {
        match self.node {
            FileTree(FileSystemObject::Directory(d)) => Ok(MemoryTreeEntries(d.entries.iter())),
            _ => Err(MemorySourceError::NotADirectory),
        }
    }

    async fn open(&self, name: &str) -> Result<Self, Self::Error> {
        match self.node {
            FileTree(FileSystemObject::Directory(Directory { entries })) => {
                let child = entries
                    .get(name)
                    .ok_or_else(|| MemorySourceError::NotFound(name.to_owned()))?;
                Ok(MemoryTreeSource { node: child })
            }
            _ => Err(MemorySourceError::NotADirectory),
        }
    }
}

/// Iterator over children of a [`MemoryTreeSource`] directory.
pub struct MemoryTreeEntries<'a>(std::collections::btree_map::Iter<'a, String, Box<MemoryTree>>);

impl<'a> futures_core::Stream for MemoryTreeEntries<'a> {
    type Item = Result<
        (
            String,
            std::future::Ready<Result<MemoryTreeSource<'a>, MemorySourceError>>,
        ),
        MemorySourceError,
    >;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        std::task::Poll::Ready(self.0.next().map(|(name, child)| {
            Ok((
                name.clone(),
                std::future::ready(Ok(MemoryTreeSource { node: child })),
            ))
        }))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}

fn stat_of(node: &MemoryTree) -> Stat {
    match node {
        FileTree(FileSystemObject::Regular(Regular {
            executable,
            contents,
        })) => Stat {
            file_type: FileType::Regular,
            file_size: Some(contents.len() as u64),
            executable: *executable,
        },
        FileTree(FileSystemObject::Directory(_)) => Stat {
            file_type: FileType::Directory,
            file_size: None,
            executable: false,
        },
        FileTree(FileSystemObject::Symlink(_)) => Stat {
            file_type: FileType::Symlink,
            file_size: None,
            executable: false,
        },
    }
}
