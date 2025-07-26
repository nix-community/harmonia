use crate::error::ProtocolError;
use crate::protocol::ProtocolVersion;
use crate::protocol::types::{GCOptions, GCResult, GCRoot, VerifyStoreRequest};
use crate::serialization::{Deserialize, Serialize};
use harmonia_store_core::StorePath;
use std::collections::{BTreeMap, BTreeSet};
use tokio::io::{AsyncRead, AsyncWrite};

impl Serialize for GCOptions {
    async fn serialize<W: AsyncWrite + Unpin>(
        &self,
        writer: &mut W,
        version: ProtocolVersion,
    ) -> Result<(), ProtocolError> {
        // Write GC action as u64
        self.operation.as_u64().serialize(writer, version).await?;

        // Write paths to delete
        self.paths_to_delete.serialize(writer, version).await?;

        // Write ignore liveness as bool
        self.ignore_liveness.serialize(writer, version).await?;

        // Write max freed
        self.max_freed.serialize(writer, version).await?;

        // Write 3 obsolete fields as 0
        0u64.serialize(writer, version).await?;
        0u64.serialize(writer, version).await?;
        0u64.serialize(writer, version).await?;

        Ok(())
    }
}

impl Deserialize for GCResult {
    async fn deserialize<R: AsyncRead + Unpin>(
        reader: &mut R,
        version: ProtocolVersion,
    ) -> Result<Self, ProtocolError> {
        // Read paths as strings and convert to StorePath
        let path_strings = <Vec<String>>::deserialize(reader, version).await?;
        let mut deleted_paths = BTreeSet::new();
        for path_str in path_strings {
            // Store paths come as full paths from the daemon
            let path = StorePath::from_bytes(path_str.as_bytes());
            deleted_paths.insert(path);
        }

        // Read bytes freed
        let bytes_freed = u64::deserialize(reader, version).await?;

        // Read obsolete field
        u64::deserialize(reader, version).await?;

        Ok(GCResult {
            deleted_paths,
            bytes_freed,
        })
    }
}

impl Deserialize for BTreeMap<StorePath, GCRoot> {
    async fn deserialize<R: AsyncRead + Unpin>(
        reader: &mut R,
        version: ProtocolVersion,
    ) -> Result<Self, ProtocolError> {
        // Read the number of entries
        let count = u64::deserialize(reader, version).await?;

        let mut roots = BTreeMap::new();
        for _ in 0..count {
            // Read link path (string)
            let link = String::deserialize(reader, version).await?;

            // Read store path
            let target = StorePath::deserialize(reader, version).await?;

            // Determine if the root is censored based on the link
            let root = if link.contains("/proc/") && link.contains("/exe") {
                GCRoot::Censored
            } else {
                GCRoot::Path(target.clone())
            };

            roots.insert(target, root);
        }

        Ok(roots)
    }
}

impl Serialize for VerifyStoreRequest {
    async fn serialize<W: AsyncWrite + Unpin>(
        &self,
        writer: &mut W,
        version: ProtocolVersion,
    ) -> Result<(), ProtocolError> {
        self.check_contents.serialize(writer, version).await?;
        self.repair.serialize(writer, version).await?;
        Ok(())
    }
}
