use futures_util::StreamExt;

use harmonia_file_core::{Directory, FileSystemObject, FileTree, Regular, Symlink};

use crate::{FileSystemSource, FileType, Stat};

/// An opaque placeholder used in shallow listings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Opaque;

/// A shallow (one-level) file tree — directory children are [`Opaque`].
pub type ShallowTree<C> = FileSystemObject<C, Opaque>;

/// Produce a fully recursive listing from a [`FileSystemSource`].
pub async fn list_deep(
    source: &impl FileSystemSource,
) -> Result<FileTree<Stat>, Box<dyn std::error::Error>> {
    let stat = source.lstat().await.map_err(box_err)?;
    match stat.file_type {
        FileType::Regular => Ok(FileTree(FileSystemObject::Regular(Regular {
            executable: stat.executable,
            contents: stat,
        }))),
        FileType::Symlink => {
            let target = source.read_link().await.map_err(box_err)?;
            Ok(FileTree(FileSystemObject::Symlink(Symlink { target })))
        }
        FileType::Directory => {
            let mut entries = std::collections::BTreeMap::new();
            let mut stream = source.entries().await.map_err(box_err)?;
            while let Some(item) = stream.next().await {
                let (name, child_thunk) = item.map_err(box_err)?;
                let child = child_thunk.await.map_err(box_err)?;
                let listing = Box::pin(list_deep(&child)).await?;
                entries.insert(name, Box::new(listing));
            }
            Ok(FileTree(FileSystemObject::Directory(Directory { entries })))
        }
    }
}

/// Produce a shallow (one-level) listing from a [`FileSystemSource`].
///
/// Directory children are represented as [`Opaque`] placeholders.
pub async fn list_shallow(
    source: &impl FileSystemSource,
) -> Result<ShallowTree<Stat>, Box<dyn std::error::Error>> {
    let stat = source.lstat().await.map_err(box_err)?;
    match stat.file_type {
        FileType::Regular => Ok(FileSystemObject::Regular(Regular {
            executable: stat.executable,
            contents: stat,
        })),
        FileType::Symlink => {
            let target = source.read_link().await.map_err(box_err)?;
            Ok(FileSystemObject::Symlink(Symlink { target }))
        }
        FileType::Directory => {
            let mut entries = std::collections::BTreeMap::new();
            let mut stream = source.entries().await.map_err(box_err)?;
            while let Some(item) = stream.next().await {
                let (name, _child) = item.map_err(box_err)?;
                entries.insert(name, Opaque);
            }
            Ok(FileSystemObject::Directory(Directory { entries }))
        }
    }
}

fn box_err(e: impl std::error::Error + 'static) -> Box<dyn std::error::Error> {
    Box::new(e)
}
