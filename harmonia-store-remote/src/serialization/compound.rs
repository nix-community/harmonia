use crate::error::ProtocolError;
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
        Ok(StorePath::new(String::deserialize(reader, version).await?))
    }
}

impl Deserialize for Vec<StorePath> {
    async fn deserialize<R: AsyncRead + Unpin>(
        reader: &mut R,
        version: ProtocolVersion,
    ) -> Result<Self, ProtocolError> {
        let len = u64::deserialize(reader, version).await?;

        if len > MAX_STRING_LIST_SIZE {
            return Err(ProtocolError::StringListTooLong {
                length: len,
                max: MAX_STRING_LIST_SIZE,
            });
        }

        let mut result = Vec::with_capacity(len as usize);
        for _ in 0..len {
            result.push(StorePath::deserialize(reader, version).await?);
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
            None => String::new().serialize(writer, version).await?,
            Some(path) => path.as_str().to_string().serialize(writer, version).await?,
        }

        // Serialize hash
        self.hash.serialize(writer, version).await?;

        // Serialize references
        let ref_strings: Vec<String> = self
            .references
            .iter()
            .map(|p| p.as_str().to_string())
            .collect();
        ref_strings.serialize(writer, version).await?;

        // Serialize registration time
        self.registration_time.serialize(writer, version).await?;

        // Serialize nar size
        self.nar_size.serialize(writer, version).await?;

        // Serialize ultimate flag
        self.ultimate.serialize(writer, version).await?;

        // Serialize signatures
        self.signatures.serialize(writer, version).await?;

        // Serialize content address (empty string if None)
        match &self.content_address {
            None => String::new().serialize(writer, version).await?,
            Some(ca) => ca.serialize(writer, version).await?,
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
        let deriver_str = String::deserialize(reader, version).await?;
        let deriver = if deriver_str.is_empty() {
            None
        } else {
            Some(StorePath::new(deriver_str))
        };

        // Deserialize hash
        let hash = String::deserialize(reader, version).await?;

        // Deserialize references
        let ref_strings = Vec::<String>::deserialize(reader, version).await?;
        let references: Vec<StorePath> = ref_strings.into_iter().map(StorePath::new).collect();

        // Deserialize registration time
        let registration_time = u64::deserialize(reader, version).await?;

        // Deserialize nar size
        let nar_size = u64::deserialize(reader, version).await?;

        // Deserialize ultimate flag
        let ultimate = bool::deserialize(reader, version).await?;

        // Deserialize signatures
        let signatures = Vec::<String>::deserialize(reader, version).await?;

        // Deserialize content address
        let ca_str = String::deserialize(reader, version).await?;
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
