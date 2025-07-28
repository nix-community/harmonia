use crate::error::ProtocolError;
use crate::protocol::ProtocolVersion;
use crate::protocol::types::substitutable::SubstitutablePathInfo;
use crate::serialization::{Deserialize, Serialize};
use harmonia_store_core::StorePath;
use std::collections::BTreeSet;
use tokio::io::{AsyncRead, AsyncWrite};

impl Serialize for SubstitutablePathInfo {
    async fn serialize<W: AsyncWrite + Unpin>(
        &self,
        writer: &mut W,
        version: ProtocolVersion,
    ) -> Result<(), ProtocolError> {
        self.deriver.serialize(writer, version).await?;
        self.references.serialize(writer, version).await?;
        self.download_size.serialize(writer, version).await?;
        self.nar_size.serialize(writer, version).await?;
        Ok(())
    }
}

impl Deserialize for SubstitutablePathInfo {
    async fn deserialize<R: AsyncRead + Unpin>(
        reader: &mut R,
        version: ProtocolVersion,
    ) -> Result<Self, ProtocolError> {
        Ok(SubstitutablePathInfo {
            deriver: Option::<StorePath>::deserialize(reader, version).await?,
            references: BTreeSet::<StorePath>::deserialize(reader, version).await?,
            download_size: u64::deserialize(reader, version).await?,
            nar_size: u64::deserialize(reader, version).await?,
        })
    }
}
