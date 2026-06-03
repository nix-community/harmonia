//! Serde implementations for IO types that can't derive.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::listing::Opaque;
use crate::source::{FileType, Stat};

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
// Stat — flattened into Regular as `{"size": N}`
// ---------------------------------------------------------------------------

impl Serialize for Stat {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(None)?;
        if let Some(size) = self.file_size {
            map.serialize_entry("size", &size)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for Stat {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Helper {
            #[serde(rename = "type", default)]
            file_type: Option<String>,
            executable: Option<bool>,
            size: Option<u64>,
        }
        let h = Helper::deserialize(deserializer)?;
        let file_type = match h.file_type.as_deref() {
            Some("regular") => FileType::Regular,
            Some("directory") => FileType::Directory,
            Some("symlink") => FileType::Symlink,
            _ => FileType::Regular,
        };
        Ok(Stat {
            file_type,
            file_size: h.size,
            executable: h.executable.unwrap_or(false),
        })
    }
}
