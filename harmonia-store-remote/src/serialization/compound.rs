use crate::error::{IoErrorContext, ProtocolError};
use crate::protocol::{MAX_STRING_SIZE, ProtocolVersion, StorePath, ValidPathInfo};
use crate::serialization::{Deserialize, Serialize};
use harmonia_store_core::{ContentAddress, NarSignature};
use std::collections::BTreeSet;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};

impl Serialize for StorePath {
    async fn serialize<W: AsyncWrite + Unpin>(
        &self,
        writer: &mut W,
        version: ProtocolVersion,
    ) -> Result<(), ProtocolError> {
        self.as_bytes().serialize(writer, version).await
    }
}

impl Deserialize for StorePath {
    async fn deserialize<R: AsyncRead + Unpin>(
        reader: &mut R,
        version: ProtocolVersion,
    ) -> Result<Self, ProtocolError> {
        // Read length
        let len = u64::deserialize(reader, version)
            .await
            .io_context("Failed to read StorePath length")?;

        if len > MAX_STRING_SIZE {
            return Err(ProtocolError::StringTooLong {
                length: len,
                max: MAX_STRING_SIZE,
            });
        }

        // Align to the next multiple of 8
        let aligned_len = (len + 7) & !7;
        let mut buf = vec![0; aligned_len as usize];
        reader
            .read_exact(&mut buf)
            .await
            .io_context("Failed to read StorePath data")?;

        buf.truncate(len as usize);
        Ok(StorePath::new(buf))
    }
}

impl Serialize for ValidPathInfo {
    async fn serialize<W: AsyncWrite + Unpin>(
        &self,
        writer: &mut W,
        version: ProtocolVersion,
    ) -> Result<(), ProtocolError> {
        // Serialize deriver (empty bytes if None)
        match &self.deriver {
            None => (&[] as &[u8])
                .serialize(writer, version)
                .await
                .io_context("Failed to write empty deriver")?,
            Some(path) => path
                .serialize(writer, version)
                .await
                .io_context("Failed to write deriver")?,
        }

        // Serialize hash as hex string bytes (nix-daemon compatibility)
        self.hash
            .to_hex()
            .serialize(writer, version)
            .await
            .io_context("Failed to write hash")?;

        // Serialize references
        self.references
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

        // Serialize content address as raw bytes (empty if None)
        // We need to write as Vec<u8> to match the deserializer's expectation
        let ca_bytes: Vec<u8> = match &self.content_address {
            None => Vec::new(),
            Some(ca) => {
                // Build the byte representation without going through String
                match ca {
                    ContentAddress::Text { hash } => {
                        let mut bytes = Vec::new();
                        bytes.extend_from_slice(b"text:");
                        bytes.extend_from_slice(hash.algo.name().as_bytes());
                        bytes.push(b':');
                        bytes.extend_from_slice(&hash.to_hex());
                        bytes
                    }
                    ContentAddress::Fixed { method, hash } => {
                        let mut bytes = Vec::new();
                        bytes.extend_from_slice(b"fixed:");
                        if matches!(method, harmonia_store_core::FileIngestionMethod::Recursive) {
                            bytes.extend_from_slice(b"r:");
                        }
                        bytes.extend_from_slice(hash.algo.name().as_bytes());
                        bytes.push(b':');
                        bytes.extend_from_slice(&hash.to_hex());
                        bytes
                    }
                }
            }
        };
        ca_bytes
            .serialize(writer, version)
            .await
            .io_context("Failed to write content_address")?;

        Ok(())
    }
}

impl Deserialize for ValidPathInfo {
    async fn deserialize<R: AsyncRead + Unpin>(
        reader: &mut R,
        version: ProtocolVersion,
    ) -> Result<Self, ProtocolError> {
        // Deserialize deriver
        let deriver_bytes = Vec::<u8>::deserialize(reader, version)
            .await
            .io_context("Failed to read deriver")?;
        let deriver = if deriver_bytes.is_empty() {
            None
        } else {
            Some(StorePath::new(deriver_bytes))
        };

        // Deserialize hash (comes as hex string bytes from nix-daemon)
        let hash_bytes = Vec::<u8>::deserialize(reader, version)
            .await
            .io_context("Failed to read hash")?;
        // Assume SHA256 for nix-daemon compatibility
        let hash = harmonia_store_core::Hash::from_hex_bytes(
            harmonia_store_core::HashAlgo::Sha256,
            &hash_bytes,
        )
        .map_err(|e| ProtocolError::DaemonError {
            message: format!("Failed to parse hash: {e}"),
        })?;

        // Deserialize references
        let references = BTreeSet::<StorePath>::deserialize(reader, version)
            .await
            .io_context("Failed to read references")?;

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
        let signatures = BTreeSet::<NarSignature>::deserialize(reader, version)
            .await
            .io_context("Failed to read signatures")?;

        // Deserialize content address
        let ca_bytes = <Vec<u8>>::deserialize(reader, version)
            .await
            .io_context("Failed to read content_address")?;
        let content_address = if ca_bytes.is_empty() {
            None
        } else {
            Some(
                ContentAddress::parse(&ca_bytes).map_err(|e| ProtocolError::DaemonError {
                    message: format!("Failed to parse content address: {e}"),
                })?,
            )
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
