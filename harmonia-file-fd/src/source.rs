use cap_tokio::fs::{Dir, Metadata};
use harmonia_file_io_pure::{FileSystemSource, FileType, Stat};

use crate::mmap;

/// A node in the filesystem acting as a [`FileSystemSource`].
///
/// Navigation uses `openat`/`fstatat` syscalls — no path assembly, no
/// symlink following on intermediate components.
#[derive(Debug)]
pub struct DirSource(pub(crate) DirSourceInner);

#[derive(Debug)]
pub(crate) enum DirSourceInner {
    /// A directory — we hold an open `Dir` handle.
    Dir { dir: Dir, meta: Metadata },
    /// A file or symlink — we hold the parent dir and child name.
    Entry {
        parent: Dir,
        name: String,
        meta: Metadata,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum DirSourceError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("not a regular file")]
    NotAFile,
    #[error("not a symlink")]
    NotASymlink,
    #[error("not a directory")]
    NotADirectory,
}

impl DirSource {
    /// Open a directory as a source.
    pub async fn open(dir: Dir) -> Result<Self, DirSourceError> {
        let meta = dir.dir_metadata().await?;
        Ok(Self(DirSourceInner::Dir { dir, meta }))
    }

    pub(crate) fn meta(&self) -> &Metadata {
        match &self.0 {
            DirSourceInner::Dir { meta, .. } => meta,
            DirSourceInner::Entry { meta, .. } => meta,
        }
    }

    /// Get or open the underlying `Dir` handle. For `Dir` variants,
    /// returns the existing handle. For `Entry` variants pointing to a
    /// directory, opens it from the parent.
    pub(crate) async fn open_dir(&self) -> Result<Dir, DirSourceError> {
        match &self.0 {
            DirSourceInner::Dir { dir, .. } => Ok(dir.clone()),
            DirSourceInner::Entry { parent, name, meta } if meta.is_dir() => {
                Ok(parent.open_dir(name).await?)
            }
            _ => Err(DirSourceError::NotADirectory),
        }
    }
}

/// Reader returned by [`DirSource::read_file`].
///
/// Small files are read via `cap_tokio::fs::File` (buffered async IO).
/// Large files (above [`mmap::MMAP_THRESHOLD`]) are memory-mapped for
/// zero-copy reads.
pub enum FileReader {
    /// Normal async file read.
    File(cap_tokio::fs::File),
    /// Memory-mapped file, wrapped in a cursor for `AsyncRead`.
    Mmap(std::io::Cursor<mmap::MappedFile>),
}

impl tokio::io::AsyncRead for FileReader {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            FileReader::File(f) => std::pin::Pin::new(f).poll_read(cx, buf),
            FileReader::Mmap(cursor) => {
                let slice = cursor.get_ref().as_slice();
                let pos = cursor.position() as usize;
                let remaining = &slice[pos..];
                let n = remaining.len().min(buf.remaining());
                buf.put_slice(&remaining[..n]);
                cursor.set_position((pos + n) as u64);
                std::task::Poll::Ready(Ok(()))
            }
        }
    }
}

impl FileSystemSource for DirSource {
    type Error = DirSourceError;
    type Reader = FileReader;
    type Child = Self;
    type ChildThunk = std::future::Ready<Result<Self, DirSourceError>>;
    type Entries<'a> = DirSourceEntries;

    async fn lstat(&self) -> Result<Stat, Self::Error> {
        let meta = self.meta();
        let file_type = if meta.is_dir() {
            FileType::Directory
        } else if meta.is_symlink() {
            FileType::Symlink
        } else {
            FileType::Regular
        };
        let file_size = if file_type == FileType::Regular {
            Some(meta.len())
        } else {
            None
        };
        Ok(Stat {
            file_type,
            file_size,
            executable: super::is_executable(meta),
        })
    }

    async fn read_file(&self) -> Result<Self::Reader, Self::Error> {
        match &self.0 {
            DirSourceInner::Entry { parent, name, meta }
                if !meta.is_dir() && !meta.is_symlink() =>
            {
                let size = meta.len();
                let file = parent.open(name).await?;
                if size > mmap::MMAP_THRESHOLD {
                    let mapped = mmap::MappedFile::from_fd(&file, size)?;
                    Ok(FileReader::Mmap(std::io::Cursor::new(mapped)))
                } else {
                    Ok(FileReader::File(file))
                }
            }
            _ => Err(DirSourceError::NotAFile),
        }
    }

    async fn read_link(&self) -> Result<String, Self::Error> {
        match &self.0 {
            DirSourceInner::Entry { parent, name, meta } if meta.is_symlink() => {
                let target = parent.read_link(name).await?;
                target.into_os_string().into_string().map_err(|t| {
                    std::io::Error::other(format!("non-UTF-8 symlink target: {t:?}")).into()
                })
            }
            _ => Err(DirSourceError::NotASymlink),
        }
    }

    async fn entries(&self) -> Result<Self::Entries<'_>, Self::Error> {
        let dir = self.open_dir().await?;
        let read_dir = dir.read_dir(".").await?;
        let mut names = Vec::new();
        for entry in read_dir {
            let entry = entry?;
            names.push(
                entry
                    .file_name()
                    .into_string()
                    .map_err(|n| std::io::Error::other(format!("non-UTF-8 filename: {n:?}")))?,
            );
        }
        names.sort();
        let mut children = Vec::with_capacity(names.len());
        for name in names {
            let meta = dir.symlink_metadata(&name).await?;
            let parent = dir.clone();
            children.push((
                name.clone(),
                DirSource(DirSourceInner::Entry { parent, name, meta }),
            ));
        }
        Ok(DirSourceEntries(children.into_iter()))
    }

    async fn open(&self, name: &str) -> Result<Self, Self::Error> {
        let dir = self.open_dir().await?;
        let meta = dir.symlink_metadata(name).await?;
        let parent = dir.clone();
        Ok(DirSource(DirSourceInner::Entry {
            parent,
            name: name.to_owned(),
            meta,
        }))
    }
}

/// Pre-collected children of a [`DirSource`] directory, yielded as a stream.
///
/// Each item is a `(name, thunk)` where the thunk is a `Ready` future
/// — the `DirSource` entry is already constructed (with cached metadata)
/// but the underlying directory handle is NOT opened until someone calls
/// `entries()`/`open()` on the child.
pub struct DirSourceEntries(std::vec::IntoIter<(String, DirSource)>);

impl futures_core::Stream for DirSourceEntries {
    type Item = Result<
        (
            String,
            std::future::Ready<Result<DirSource, DirSourceError>>,
        ),
        DirSourceError,
    >;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        std::task::Poll::Ready(
            self.0
                .next()
                .map(|(name, child)| Ok((name, std::future::ready(Ok(child))))),
        )
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}
