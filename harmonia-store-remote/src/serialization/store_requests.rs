use crate::error::{IoErrorContext, ProtocolError};
use crate::protocol::{
    ProtocolVersion,
    types::{AddSignaturesRequest, AddTextToStoreRequest},
};
use crate::serialization::Serialize;
use tokio::io::AsyncWrite;

impl<'a> Serialize for AddTextToStoreRequest<'a> {
    async fn serialize<W: AsyncWrite + Unpin>(
        &self,
        writer: &mut W,
        version: ProtocolVersion,
    ) -> Result<(), ProtocolError> {
        // The protocol expects these fields in this exact order
        self.name
            .serialize(writer, version)
            .await
            .io_context("Failed to write name")?;
        self.content
            .serialize(writer, version)
            .await
            .io_context("Failed to write content")?;
        self.references
            .serialize(writer, version)
            .await
            .io_context("Failed to write references")?;
        self.repair
            .serialize(writer, version)
            .await
            .io_context("Failed to write repair flag")?;
        Ok(())
    }
}

impl<'a> Serialize for AddSignaturesRequest<'a> {
    async fn serialize<W: AsyncWrite + Unpin>(
        &self,
        writer: &mut W,
        version: ProtocolVersion,
    ) -> Result<(), ProtocolError> {
        // The protocol expects path first, then signatures
        self.path
            .serialize(writer, version)
            .await
            .io_context("Failed to write path")?;
        self.signatures
            .serialize(writer, version)
            .await
            .io_context("Failed to write signatures")?;
        Ok(())
    }
}
