use tokio::io::AsyncWrite;

use harmonia_file_core::{Directory, FileSystemObject, FileTree, MemoryTree, Regular, Symlink};

// ---------------------------------------------------------------------------
// Traits
// ---------------------------------------------------------------------------

/// Write-side interface: choose what kind of node to create at this position.
///
/// One level at a time — `create_directory` returns a [`DirectorySink`]
/// for populating children, `create_regular_file` returns a [`RegularFileSink`]
/// for streaming contents.
// TODO: desugar to `fn foo() -> impl Future<...> + Send` once we need
// to spawn tasks that hold these futures across await points.
#[allow(async_fn_in_trait)]
pub trait FileSystemSink: Sized {
    type Error: std::error::Error;
    type Directory: DirectorySink<Error = Self::Error>;
    type File: RegularFileSink<Error = Self::Error>;

    async fn create_directory(self) -> Result<Self::Directory, Self::Error>;
    async fn create_regular_file(self, executable: bool) -> Result<Self::File, Self::Error>;
    async fn create_symlink(self, target: &str) -> Result<(), Self::Error>;
}

/// Populate a directory's children one at a time.
#[allow(async_fn_in_trait)]
pub trait DirectorySink: Sized {
    type Error: std::error::Error;
    type Child<'a>: FileSystemSink<Error = Self::Error>
    where
        Self: 'a;

    async fn create_child(&mut self, name: &str) -> Result<Self::Child<'_>, Self::Error>;
}

/// Sink for streaming contents into a regular file.
#[allow(async_fn_in_trait)]
pub trait RegularFileSink: AsyncWrite + Unpin {
    type Error: std::error::Error;

    #[allow(async_fn_in_trait)]
    async fn preallocate(&mut self, _size: u64) -> Result<(), Self::Error> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// In-memory implementation
// ---------------------------------------------------------------------------

/// Errors from in-memory tree building.
#[derive(Debug, thiserror::Error)]
pub enum MemoryBuildError {
    #[error("IO error reading contents: {0}")]
    Io(#[from] std::io::Error),
}

fn placeholder() -> MemoryTree {
    FileTree(FileSystemObject::Directory(Directory {
        entries: Default::default(),
    }))
}

/// Builder that owns a [`MemoryTree`] and provides a [`FileSystemSink`]
/// for populating it.
pub struct MemoryTreeBuilder {
    root: MemoryTree,
}

impl MemoryTreeBuilder {
    pub fn new() -> Self {
        Self {
            root: placeholder(),
        }
    }

    /// Get a sink for the root slot.
    pub fn sink(&mut self) -> MemorySlotSink<'_> {
        MemorySlotSink(&mut self.root)
    }

    /// Consume the builder and return the finished tree.
    pub fn build(self) -> MemoryTree {
        self.root
    }
}

impl Default for MemoryTreeBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// A mutable reference to a tree node that will be overwritten.
pub struct MemorySlotSink<'a>(&'a mut MemoryTree);

impl<'a> FileSystemSink for MemorySlotSink<'a> {
    type Error = MemoryBuildError;
    type Directory = MemoryDirSink<'a>;
    type File = MemoryFileSink<'a>;

    async fn create_directory(self) -> Result<Self::Directory, Self::Error> {
        *self.0 = FileTree(FileSystemObject::Directory(Directory {
            entries: Default::default(),
        }));
        Ok(MemoryDirSink(self.0))
    }

    async fn create_regular_file(self, executable: bool) -> Result<Self::File, Self::Error> {
        Ok(MemoryFileSink {
            slot: self.0,
            executable,
            data: Vec::new(),
        })
    }

    async fn create_symlink(self, target: &str) -> Result<(), Self::Error> {
        *self.0 = FileTree(FileSystemObject::Symlink(Symlink {
            target: target.to_owned(),
        }));
        Ok(())
    }
}

/// A directory being populated with children.
pub struct MemoryDirSink<'a>(&'a mut MemoryTree);

impl<'a> DirectorySink for MemoryDirSink<'a> {
    type Error = MemoryBuildError;
    type Child<'b>
        = MemorySlotSink<'b>
    where
        Self: 'b;

    async fn create_child(&mut self, name: &str) -> Result<MemorySlotSink<'_>, Self::Error> {
        let FileTree(FileSystemObject::Directory(dir)) = &mut *self.0 else {
            unreachable!("MemoryDirSink always wraps a directory");
        };
        dir.entries
            .entry(name.to_owned())
            .or_insert_with(|| Box::new(placeholder()));
        let child = dir.entries.get_mut(name).unwrap();
        Ok(MemorySlotSink(child.as_mut()))
    }
}

/// A regular file being written.
pub struct MemoryFileSink<'a> {
    slot: &'a mut MemoryTree,
    executable: bool,
    data: Vec<u8>,
}

impl<'a> AsyncWrite for MemoryFileSink<'a> {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        self.data.extend_from_slice(buf);
        std::task::Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
}

impl<'a> RegularFileSink for MemoryFileSink<'a> {
    type Error = MemoryBuildError;
}

impl<'a> Drop for MemoryFileSink<'a> {
    fn drop(&mut self) {
        let data = std::mem::take(&mut self.data);
        *self.slot = FileTree(FileSystemObject::Regular(Regular {
            executable: self.executable,
            contents: data,
        }));
    }
}
