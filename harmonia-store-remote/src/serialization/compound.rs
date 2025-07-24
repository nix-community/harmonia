use crate::error::{IoErrorContext, ProtocolError};
use crate::protocol::{ProtocolVersion, StorePath, ValidPathInfo, MAX_STRING_LIST_SIZE};
use crate::serialization::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncWrite};

impl Serialize for StorePath {
    async fn serialize<W: AsyncWrite + Unpin>(
        &self,
        writer: &mut W,
        version: ProtocolVersion,
    ) -> Result<(), ProtocolError> {
        self.as_str().to_string().serialize(writer, version).await
    }
}

impl Deserialize for StorePath {
    async fn deserialize<R: AsyncRead + Unpin>(
        reader: &mut R,
        version: ProtocolVersion,
    ) -> Result<Self, ProtocolError> {
        Ok(StorePath::new(
            String::deserialize(reader, version)
                .await
                .io_context("Failed to deserialize StorePath")?,
        ))
    }
}

impl Deserialize for Vec<StorePath> {
    async fn deserialize<R: AsyncRead + Unpin>(
        reader: &mut R,
        version: ProtocolVersion,
    ) -> Result<Self, ProtocolError> {
        let len = u64::deserialize(reader, version)
            .await
            .io_context("Failed to read Vec<StorePath> length")?;

        if len > MAX_STRING_LIST_SIZE {
            return Err(ProtocolError::StringListTooLong {
                length: len,
                max: MAX_STRING_LIST_SIZE,
            });
        }

        let mut result = Vec::with_capacity(len as usize);
        for i in 0..len {
            result.push(
                StorePath::deserialize(reader, version)
                    .await
                    .io_context(format!("Failed to read Vec<StorePath> item {}", i))?,
            );
        }
        Ok(result)
    }
}

impl Serialize for ValidPathInfo {
    async fn serialize<W: AsyncWrite + Unpin>(
        &self,
        writer: &mut W,
        version: ProtocolVersion,
    ) -> Result<(), ProtocolError> {
        // Serialize deriver (empty string if None)
        match &self.deriver {
            None => String::new()
                .serialize(writer, version)
                .await
                .io_context("Failed to write empty deriver")?,
            Some(path) => path
                .as_str()
                .to_string()
                .serialize(writer, version)
                .await
                .io_context("Failed to write deriver")?,
        }

        // Serialize hash
        self.hash
            .serialize(writer, version)
            .await
            .io_context("Failed to write hash")?;

        // Serialize references
        let ref_strings: Vec<String> = self
            .references
            .iter()
            .map(|p| p.as_str().to_string())
            .collect();
        ref_strings
            .serialize(writer, version)
            .await
            .io_context("Failed to write references")?;

        // Serialize registration time
        self.registration_time
            .serialize(writer, version)
            .await
            .io_context("Failed to write registration_time")?;

        // Serialize nar size
        self.nar_size
            .serialize(writer, version)
            .await
            .io_context("Failed to write nar_size")?;

        // Serialize ultimate flag
        self.ultimate
            .serialize(writer, version)
            .await
            .io_context("Failed to write ultimate")?;

        // Serialize signatures
        self.signatures
            .serialize(writer, version)
            .await
            .io_context("Failed to write signatures")?;

        // Serialize content address (empty string if None)
        match &self.content_address {
            None => String::new()
                .serialize(writer, version)
                .await
                .io_context("Failed to write empty content_address")?,
            Some(ca) => ca
                .serialize(writer, version)
                .await
                .io_context("Failed to write content_address")?,
        }

        Ok(())
    }
}

impl Deserialize for ValidPathInfo {
    async fn deserialize<R: AsyncRead + Unpin>(
        reader: &mut R,
        version: ProtocolVersion,
    ) -> Result<Self, ProtocolError> {
        // Deserialize deriver
        let deriver_str = String::deserialize(reader, version)
            .await
            .io_context("Failed to read deriver")?;
        let deriver = if deriver_str.is_empty() {
            None
        } else {
            Some(StorePath::new(deriver_str))
        };

        // Deserialize hash
        let hash = String::deserialize(reader, version)
            .await
            .io_context("Failed to read hash")?;

        // Deserialize references
        let ref_strings = Vec::<String>::deserialize(reader, version)
            .await
            .io_context("Failed to read references")?;
        let references: Vec<StorePath> = ref_strings.into_iter().map(StorePath::new).collect();

        // Deserialize registration time
        let registration_time = u64::deserialize(reader, version)
            .await
            .io_context("Failed to read registration_time")?;

        // Deserialize nar size
        let nar_size = u64::deserialize(reader, version)
            .await
            .io_context("Failed to read nar_size")?;

        // Deserialize ultimate flag
        let ultimate = bool::deserialize(reader, version)
            .await
            .io_context("Failed to read ultimate")?;

        // Deserialize signatures
        let signatures = Vec::<String>::deserialize(reader, version)
            .await
            .io_context("Failed to read signatures")?;

        // Deserialize content address
        let ca_str = String::deserialize(reader, version)
            .await
            .io_context("Failed to read content_address")?;
        let content_address = if ca_str.is_empty() {
            None
        } else {
            Some(ca_str)
        };

        Ok(ValidPathInfo {
            deriver,
            hash,
            references,
            registration_time,
            nar_size,
            ultimate,
            signatures,
            content_address,
        })
    }
}
