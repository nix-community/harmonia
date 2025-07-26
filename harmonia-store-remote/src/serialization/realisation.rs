use crate::error::ProtocolError;
use crate::protocol::ProtocolVersion;
use crate::serialization::{Deserialize, Serialize};
use harmonia_store_core::{NarSignature, StorePath};
use std::collections::BTreeSet;
use tokio::io::{AsyncRead, AsyncWrite};

impl Serialize for DrvOutputId {
    async fn serialize<W: AsyncWrite + Unpin>(
        &self,
        writer: &mut W,
        version: ProtocolVersion,
    ) -> Result<(), ProtocolError> {
        self.drv_hash.serialize(writer, version).await?;
        self.output_name.serialize(writer, version).await?;
        Ok(())
    }
}

impl Deserialize for DrvOutputId {
    async fn deserialize<R: AsyncRead + Unpin>(
        reader: &mut R,
        version: ProtocolVersion,
    ) -> Result<Self, ProtocolError> {
        Ok(DrvOutputId {
            drv_hash: Vec::<u8>::deserialize(reader, version).await?,
            output_name: Vec::<u8>::deserialize(reader, version).await?,
        })
    }
}

impl Serialize for Realisation {
    async fn serialize<W: AsyncWrite + Unpin>(
        &self,
        writer: &mut W,
        version: ProtocolVersion,
    ) -> Result<(), ProtocolError> {
        self.id.serialize(writer, version).await?;
        self.out_path.serialize(writer, version).await?;
        self.signatures.serialize(writer, version).await?;
        Ok(())
    }
}

impl Deserialize for Realisation {
    async fn deserialize<R: AsyncRead + Unpin>(
        reader: &mut R,
        version: ProtocolVersion,
    ) -> Result<Self, ProtocolError> {
        Ok(Realisation {
            id: DrvOutputId::deserialize(reader, version).await?,
            out_path: StorePath::deserialize(reader, version).await?,
            signatures: BTreeSet::<NarSignature>::deserialize(reader, version).await?,
        })
    }
}

// Note: DaemonSettings is only used for sending, not receiving
impl Serialize for DaemonSettings {
    async fn serialize<W: AsyncWrite + Unpin>(
        &self,
        _writer: &mut W,
        _version: ProtocolVersion,
    ) -> Result<(), ProtocolError> {
        // The serialization is handled directly in set_options() method
        // because it has complex version-dependent logic
        unimplemented!("DaemonSettings serialization is handled in set_options()")
    }
}