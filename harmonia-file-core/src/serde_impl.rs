//! Serde implementations for types that can't derive.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{FileSystemObject, FileTree};

// ---------------------------------------------------------------------------
// FileTree<C> — newtype delegates to FileSystemObject
// ---------------------------------------------------------------------------

impl<C: Serialize> Serialize for FileTree<C> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de, C: DeserializeOwned> Deserialize<'de> for FileTree<C> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let inner = FileSystemObject::<C, Box<FileTree<C>>>::deserialize(deserializer)?;
        Ok(FileTree(inner))
    }
}
