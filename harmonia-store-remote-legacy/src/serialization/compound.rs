use crate::error::{IoErrorContext, ProtocolError};
use crate::protocol::{
    MAX_STRING_LIST_SIZE, MAX_STRING_SIZE, ProtocolVersion, StorePath, ValidPathInfo,
};
use crate::serialization::{Deserialize, Serialize};
use std::collections::BTreeSet;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};

impl Serialize for StorePath {
    async fn serialize<W: AsyncWrite + Unpin>(
        &self,
        writer: &mut W,
        version: ProtocolVersion,
        store_dir: &harmonia_store_core::store_path::StoreDir,
    ) -> Result<(), ProtocolError> {
        // Serialize as full path "/nix/store/hash-name"
        let full_path = format!("{}/{}", store_dir, self);
        full_path.as_bytes().serialize(writer, version, store_dir).await
    }
}

impl Deserialize for StorePath {
    async fn deserialize<R: AsyncRead + Unpin>(
        reader: &mut R,
        version: ProtocolVersion,
        store_dir: &harmonia_store_core::store_path::StoreDir,
    ) -> Result<Self, ProtocolError> {
        // Read length
        let len = u64::deserialize(reader, version, store_dir)
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

        // Parse the full path using store_dir to extract just the hash-name
        let path_str = std::str::from_utf8(&buf).map_err(|e| ProtocolError::DaemonError {
            message: format!("StorePath is not valid UTF-8: {e}"),
        })?;
        store_dir.parse(path_str).map_err(|e| ProtocolError::DaemonError {
            message: format!("Failed to parse StorePath: {e}"),
        })
    }
}

impl Deserialize for Vec<StorePath> {
    async fn deserialize<R: AsyncRead + Unpin>(
        reader: &mut R,
        version: ProtocolVersion,
        store_dir: &harmonia_store_core::store_path::StoreDir,
    ) -> Result<Self, ProtocolError> {
        let len = u64::deserialize(reader, version, store_dir)
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
                StorePath::deserialize(reader, version, store_dir)
                    .await
                    .io_context(format!("Failed to read Vec<StorePath> item {i}"))?,
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
        store_dir: &harmonia_store_core::store_path::StoreDir,
    ) -> Result<(), ProtocolError> {
        // Serialize deriver (empty bytes if None)
        match &self.deriver {
            None => (&[] as &[u8])
                .serialize(writer, version, store_dir)
                .await
                .io_context("Failed to write empty deriver")?,
            Some(path) => path
                .serialize(writer, version, store_dir)
                .await
                .io_context("Failed to write deriver")?,
        }

        // Serialize hash as hex string bytes (nix-daemon compatibility)
        self.hash
            .to_hex()
            .serialize(writer, version, store_dir)
            .await
            .io_context("Failed to write hash")?;

        // Serialize references
        self.references
            .serialize(writer, version, store_dir)
            .await
            .io_context("Failed to write references")?;

        // Serialize registration time
        self.registration_time
            .serialize(writer, version, store_dir)
            .await
            .io_context("Failed to write registration_time")?;

        // Serialize nar size
        self.nar_size
            .serialize(writer, version, store_dir)
            .await
            .io_context("Failed to write nar_size")?;

        // Serialize ultimate flag
        self.ultimate
            .serialize(writer, version, store_dir)
            .await
            .io_context("Failed to write ultimate")?;

        // Serialize signatures
        self.signatures
            .serialize(writer, version, store_dir)
            .await
            .io_context("Failed to write signatures")?;

        // Serialize content address (empty string if None)
        match &self.content_address {
            None => <Vec<u8>>::new()
                .serialize(writer, version, store_dir)
                .await
                .io_context("Failed to write empty content_address")?,
            Some(ca) => ca
                .serialize(writer, version, store_dir)
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
        store_dir: &harmonia_store_core::store_path::StoreDir,
    ) -> Result<Self, ProtocolError> {
        // Deserialize deriver
        let deriver_bytes = Vec::<u8>::deserialize(reader, version, store_dir)
            .await
            .io_context("Failed to read deriver")?;
        let deriver = if deriver_bytes.is_empty() {
            None
        } else {
            let deriver_str = std::str::from_utf8(&deriver_bytes).map_err(|e| ProtocolError::DaemonError {
                message: format!("Deriver StorePath is not valid UTF-8: {e}"),
            })?;
            Some(
                store_dir.parse(deriver_str).map_err(|e| ProtocolError::DaemonError {
                    message: format!("Failed to parse deriver StorePath: {e}"),
                })?,
            )
        };

        // Deserialize hash (comes as hex string bytes from nix-daemon)
        let hash_bytes = Vec::<u8>::deserialize(reader, version, store_dir)
            .await
            .io_context("Failed to read hash")?;
        // Decode hex to raw bytes and create Hash (assuming SHA256)
        let hash_str = String::from_utf8(hash_bytes).map_err(|e| ProtocolError::DaemonError {
            message: format!("Failed to decode hash as UTF-8: {e}"),
        })?;
        let raw_hash = hex::decode(&hash_str).map_err(|e| ProtocolError::DaemonError {
            message: format!("Failed to decode hex hash: {e}"),
        })?;
        // Create legacy Hash from raw bytes
        use harmonia_store_core_legacy::{Hash as LegacyHash, HashAlgo};
        let hash = LegacyHash::new(HashAlgo::Sha256, raw_hash)
            .map_err(|e| ProtocolError::DaemonError {
                message: format!("Failed to create hash: {e}"),
            })?;

        // Deserialize references
        let references = BTreeSet::<StorePath>::deserialize(reader, version, store_dir)
            .await
            .io_context("Failed to read references")?;

        // Deserialize registration time
        let registration_time = u64::deserialize(reader, version, store_dir)
            .await
            .io_context("Failed to read registration_time")?;

        // Deserialize nar size
        let nar_size = u64::deserialize(reader, version, store_dir)
            .await
            .io_context("Failed to read nar_size")?;

        // Deserialize ultimate flag
        let ultimate = bool::deserialize(reader, version, store_dir)
            .await
            .io_context("Failed to read ultimate")?;

        // Deserialize signatures
        let signatures = Vec::<Vec<u8>>::deserialize(reader, version, store_dir)
            .await
            .io_context("Failed to read signatures")?;

        // Deserialize content address
        let ca = <Vec<u8>>::deserialize(reader, version, store_dir)
            .await
            .io_context("Failed to read content_address")?;
        let content_address = if ca.is_empty() { None } else { Some(ca) };

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
