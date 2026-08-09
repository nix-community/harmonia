//! Async wrapper over sync `cap-std`, offloading each operation to `spawn_blocking`.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::vec::IntoIter;

use tokio::task::spawn_blocking;

pub use cap_std::AmbientAuthority;
pub use cap_std::ambient_authority;
pub use cap_std::fs::{Metadata, MetadataExt, OpenOptions};

/// Opened under a [`Dir`] capability, so a plain async [`tokio`] file suffices.
pub type File = tokio::fs::File;

/// Runs one blocking cap-std call off the async runtime and folds any join failure back into an io error.
async fn blocking<T, F>(f: F) -> io::Result<T>
where
    F: FnOnce() -> io::Result<T> + Send + 'static,
    T: Send + 'static,
{
    match spawn_blocking(f).await {
        Ok(result) => result,
        Err(join_error) => Err(io::Error::other(join_error)),
    }
}

#[derive(Clone, Debug)]
pub struct Dir(Arc<cap_std::fs::Dir>);

impl Dir {
    /// This runs once at the root, after which all navigation stays confined to the capability it returns.
    pub async fn open_ambient_dir(path: &Path, authority: AmbientAuthority) -> io::Result<Self> {
        let path = path.to_owned();
        let dir = blocking(move || cap_std::fs::Dir::open_ambient_dir(&path, authority)).await?;
        Ok(Self(Arc::new(dir)))
    }

    /// Reads the metadata of the directory this handle points at.
    pub async fn dir_metadata(&self) -> io::Result<Metadata> {
        let dir = self.0.clone();
        blocking(move || dir.dir_metadata()).await
    }

    /// Reads a child's metadata without following it when it happens to be a symlink.
    pub async fn symlink_metadata(&self, name: &str) -> io::Result<Metadata> {
        let dir = self.0.clone();
        let name = name.to_owned();
        blocking(move || dir.symlink_metadata(name)).await
    }

    /// Opens a child directory relative to this one.
    pub async fn open_dir(&self, name: &str) -> io::Result<Self> {
        let dir = self.0.clone();
        let name = name.to_owned();
        let opened = blocking(move || dir.open_dir(name)).await?;
        Ok(Self(Arc::new(opened)))
    }

    /// Opens a child file for reading.
    pub async fn open(&self, name: &str) -> io::Result<File> {
        let dir = self.0.clone();
        let name = name.to_owned();
        let std_file = blocking(move || dir.open(name).map(cap_std::fs::File::into_std)).await?;
        Ok(File::from_std(std_file))
    }

    /// Opens a child file with whatever options the caller chose.
    pub async fn open_with(&self, name: &str, options: &OpenOptions) -> io::Result<File> {
        let dir = self.0.clone();
        let name = name.to_owned();
        let options = options.clone();
        let std_file = blocking(move || {
            dir.open_with(name, &options)
                .map(cap_std::fs::File::into_std)
        })
        .await?;
        Ok(File::from_std(std_file))
    }

    /// Returns the link target verbatim, including absolute paths that point outside the capability.
    pub async fn read_link(&self, name: &str) -> io::Result<PathBuf> {
        let dir = self.0.clone();
        let name = name.to_owned();
        blocking(move || dir.read_link_contents(name)).await
    }

    /// Lists a directory, capturing every child name before the handle is dropped.
    pub async fn read_dir(&self, name: &str) -> io::Result<IntoIter<io::Result<DirEntry>>> {
        let dir = self.0.clone();
        let name = name.to_owned();
        blocking(move || {
            let entries = dir
                .read_dir(name)?
                .map(|entry| entry.map(|entry| DirEntry(entry.file_name())))
                .collect::<Vec<_>>();
            Ok(entries.into_iter())
        })
        .await
    }

    /// Creates a child directory relative to this one.
    pub async fn create_dir(&self, name: &str) -> io::Result<()> {
        let dir = self.0.clone();
        let name = name.to_owned();
        blocking(move || dir.create_dir(name)).await
    }

    /// Creates a symlink named `link` that points at `original`.
    pub async fn symlink(&self, original: &str, link: &str) -> io::Result<()> {
        let dir = self.0.clone();
        let original = original.to_owned();
        let link = link.to_owned();
        blocking(move || dir.symlink(original, link)).await
    }
}

/// Just the name, copied out so nothing touches [`ReadDir`](cap_std::fs::ReadDir) after [`blocking`].
pub struct DirEntry(OsString);

impl DirEntry {
    /// This hands back an owned copy because the entry only keeps the name around.
    pub fn file_name(&self) -> OsString {
        self.0.clone()
    }
}
