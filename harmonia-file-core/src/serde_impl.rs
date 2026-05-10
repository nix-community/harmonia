//! Serde implementations for types that can't derive.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{FileSystemObject, FileTree, Opaque};

// ---------------------------------------------------------------------------
// Opaque — serializes as `{}`
// ---------------------------------------------------------------------------

impl Serialize for Opaque {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        serializer.serialize_map(Some(0))?.end()
    }
}

impl<'de> Deserialize<'de> for Opaque {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let _ = <serde_json::Map<String, serde_json::Value>>::deserialize(deserializer)?;
        Ok(Opaque)
    }
}

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
        let inner: FileSystemObject<C, Box<FileTree<C>>> =
            FileSystemObject::deserialize(deserializer)?;
        Ok(FileTree(inner))
    }
}
