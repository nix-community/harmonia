use crate::error::{IoErrorContext, ProtocolError};
use crate::protocol::{MAX_STRING_LIST_SIZE, MAX_STRING_SIZE, ProtocolVersion};
use crate::serialization::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

// Implement Serialize for empty tuple (for operations with no arguments)
impl Serialize for () {
    async fn serialize<W: AsyncWrite + Unpin>(
        &self,
        _writer: &mut W,
        _version: ProtocolVersion,
    ) -> Result<(), ProtocolError> {
        // Empty tuple serializes to nothing
        Ok(())
    }
}

impl Deserialize for () {
    async fn deserialize<R: AsyncRead + Unpin>(
        _reader: &mut R,
        _version: ProtocolVersion,
    ) -> Result<Self, ProtocolError> {
        // Empty tuple reads nothing
        Ok(())
    }
}

impl Serialize for u64 {
    async fn serialize<W: AsyncWrite + Unpin>(
        &self,
        writer: &mut W,
        _version: ProtocolVersion,
    ) -> Result<(), ProtocolError> {
        writer
            .write_all(&self.to_le_bytes())
            .await
            .io_context("Failed to write u64")?;
        Ok(())
    }
}

impl Deserialize for u64 {
    async fn deserialize<R: AsyncRead + Unpin>(
        reader: &mut R,
        _version: ProtocolVersion,
    ) -> Result<Self, ProtocolError> {
        let mut buf = [0; 8];
        reader
            .read_exact(&mut buf)
            .await
            .io_context("Failed to read u64")?;
        Ok(u64::from_le_bytes(buf))
    }
}

impl Serialize for bool {
    async fn serialize<W: AsyncWrite + Unpin>(
        &self,
        writer: &mut W,
        version: ProtocolVersion,
    ) -> Result<(), ProtocolError> {
        (*self as u64).serialize(writer, version).await
    }
}

impl Deserialize for bool {
    async fn deserialize<R: AsyncRead + Unpin>(
        reader: &mut R,
        version: ProtocolVersion,
    ) -> Result<Self, ProtocolError> {
        let value = u64::deserialize(reader, version)
            .await
            .io_context("Failed to read bool")?;
        Ok(value != 0)
    }
}

impl Serialize for String {
    async fn serialize<W: AsyncWrite + Unpin>(
        &self,
        writer: &mut W,
        version: ProtocolVersion,
    ) -> Result<(), ProtocolError> {
        let len = self.len() as u64;
        len.serialize(writer, version)
            .await
            .io_context("Failed to write string length")?;
        writer
            .write_all(self.as_bytes())
            .await
            .io_context("Failed to write string data")?;

        // Padding to 8-byte boundary
        let padding_size = (8 - len % 8) % 8;
        if padding_size > 0 {
            let padding = [0u8; 8];
            writer
                .write_all(&padding[..padding_size as usize])
                .await
                .io_context("Failed to write string padding")?;
        }
        Ok(())
    }
}

// Minimal String deserializer for error messages only
impl Deserialize for String {
    async fn deserialize<R: AsyncRead + Unpin>(
        reader: &mut R,
        version: ProtocolVersion,
    ) -> Result<Self, ProtocolError> {
        let bytes = Vec::<u8>::deserialize(reader, version)
            .await
            .io_context("Failed to read String")?;
        String::from_utf8(bytes).map_err(|e| ProtocolError::DaemonError {
            message: format!("Invalid UTF-8 in daemon response: {e}"),
        })
    }
}

impl Serialize for &[u8] {
    async fn serialize<W: AsyncWrite + Unpin>(
        &self,
        writer: &mut W,
        version: ProtocolVersion,
    ) -> Result<(), ProtocolError> {
        let len = self.len() as u64;
        len.serialize(writer, version)
            .await
            .io_context("Failed to write bytes length")?;
        writer
            .write_all(self)
            .await
            .io_context("Failed to write bytes data")?;

        // Padding to 8-byte boundary
        let padding_size = (8 - len % 8) % 8;
        if padding_size > 0 {
            let padding = [0u8; 8];
            writer
                .write_all(&padding[..padding_size as usize])
                .await
                .io_context("Failed to write bytes padding")?;
        }
        Ok(())
    }
}

impl Serialize for Vec<u8> {
    async fn serialize<W: AsyncWrite + Unpin>(
        &self,
        writer: &mut W,
        version: ProtocolVersion,
    ) -> Result<(), ProtocolError> {
        self.as_slice().serialize(writer, version).await
    }
}

impl Deserialize for Vec<u8> {
    async fn deserialize<R: AsyncRead + Unpin>(
        reader: &mut R,
        version: ProtocolVersion,
    ) -> Result<Self, ProtocolError> {
        let len = u64::deserialize(reader, version)
            .await
            .io_context("Failed to read bytes length")?;

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
            .io_context("Failed to read bytes data")?;

        buf.truncate(len as usize);
        Ok(buf)
    }
}

// Generic serializer for collections that implement IntoIterator
// This handles Vec<T>, BTreeSet<T>, etc.
struct IterableSerializer;

impl IterableSerializer {
    async fn serialize_iter<'a, W, I, T>(
        iter: I,
        len: usize,
        writer: &mut W,
        version: ProtocolVersion,
    ) -> Result<(), ProtocolError>
    where
        W: AsyncWrite + Unpin,
        I: Iterator<Item = &'a T>,
        T: Serialize + 'a,
    {
        // Check max length
        if len as u64 > MAX_STRING_LIST_SIZE {
            return Err(ProtocolError::StringListTooLong {
                length: len as u64,
                max: MAX_STRING_LIST_SIZE,
            });
        }

        (len as u64)
            .serialize(writer, version)
            .await
            .io_context("Failed to write collection length")?;
        for (i, item) in iter.enumerate() {
            item.serialize(writer, version)
                .await
                .io_context(format!("Failed to write collection item {i}"))?;
        }
        Ok(())
    }
}

impl<T: Serialize> Serialize for &[T] {
    async fn serialize<W: AsyncWrite + Unpin>(
        &self,
        writer: &mut W,
        version: ProtocolVersion,
    ) -> Result<(), ProtocolError> {
        IterableSerializer::serialize_iter(self.iter(), self.len(), writer, version).await
    }
}

// We need Vec<T> deserializer for protocol compatibility
impl<T: Deserialize> Deserialize for Vec<T> {
    async fn deserialize<R: AsyncRead + Unpin>(
        reader: &mut R,
        version: ProtocolVersion,
    ) -> Result<Self, ProtocolError> {
        let len = u64::deserialize(reader, version)
            .await
            .io_context("Failed to read Vec length")?;

        if len > MAX_STRING_LIST_SIZE {
            return Err(ProtocolError::StringListTooLong {
                length: len,
                max: MAX_STRING_LIST_SIZE,
            });
        }

        let mut result = Vec::with_capacity(len as usize);
        for i in 0..len {
            result.push(
                T::deserialize(reader, version)
                    .await
                    .io_context(format!("Failed to read Vec item {i}"))?,
            );
        }
        Ok(result)
    }
}

impl<T: Serialize> Serialize for Option<T> {
    async fn serialize<W: AsyncWrite + Unpin>(
        &self,
        writer: &mut W,
        version: ProtocolVersion,
    ) -> Result<(), ProtocolError> {
        match self {
            None => 0u64
                .serialize(writer, version)
                .await
                .io_context("Failed to write Option None discriminant"),
            Some(value) => {
                1u64.serialize(writer, version)
                    .await
                    .io_context("Failed to write Option Some discriminant")?;
                value
                    .serialize(writer, version)
                    .await
                    .io_context("Failed to write Option value")
            }
        }
    }
}

impl<T: Deserialize> Deserialize for Option<T> {
    async fn deserialize<R: AsyncRead + Unpin>(
        reader: &mut R,
        version: ProtocolVersion,
    ) -> Result<Self, ProtocolError> {
        let present = u64::deserialize(reader, version)
            .await
            .io_context("Failed to read Option discriminant")?;
        if present == 0 {
            Ok(None)
        } else {
            Ok(Some(
                T::deserialize(reader, version)
                    .await
                    .io_context("Failed to read Option value")?,
            ))
        }
    }
}

impl<T: Serialize> Serialize for BTreeSet<T> {
    async fn serialize<W: AsyncWrite + Unpin>(
        &self,
        writer: &mut W,
        version: ProtocolVersion,
    ) -> Result<(), ProtocolError> {
        IterableSerializer::serialize_iter(self.iter(), self.len(), writer, version).await
    }
}

impl<T: Deserialize + Ord> Deserialize for BTreeSet<T> {
    async fn deserialize<R: AsyncRead + Unpin>(
        reader: &mut R,
        version: ProtocolVersion,
    ) -> Result<Self, ProtocolError> {
        let len = u64::deserialize(reader, version)
            .await
            .io_context("Failed to read BTreeSet length")?;

        if len > MAX_STRING_LIST_SIZE {
            return Err(ProtocolError::StringListTooLong {
                length: len,
                max: MAX_STRING_LIST_SIZE,
            });
        }

        let mut result = BTreeSet::new();
        for i in 0..len {
            let item = T::deserialize(reader, version)
                .await
                .io_context(format!("Failed to read BTreeSet item {i}"))?;
            result.insert(item);
        }

        Ok(result)
    }
}

// BTreeMap needs its own implementation because iter() yields (&K, &V) not &T
impl<K: Serialize, V: Serialize> Serialize for BTreeMap<K, V> {
    async fn serialize<W: AsyncWrite + Unpin>(
        &self,
        writer: &mut W,
        version: ProtocolVersion,
    ) -> Result<(), ProtocolError> {
        // Check max length
        if self.len() as u64 > MAX_STRING_LIST_SIZE {
            return Err(ProtocolError::StringListTooLong {
                length: self.len() as u64,
                max: MAX_STRING_LIST_SIZE,
            });
        }

        (self.len() as u64)
            .serialize(writer, version)
            .await
            .io_context("Failed to write BTreeMap length")?;
        for (i, (key, value)) in self.iter().enumerate() {
            key.serialize(writer, version)
                .await
                .io_context(format!("Failed to write BTreeMap key {i}"))?;
            value
                .serialize(writer, version)
                .await
                .io_context(format!("Failed to write BTreeMap value {i}"))?;
        }
        Ok(())
    }
}

impl<K: Deserialize + Ord, V: Deserialize> Deserialize for BTreeMap<K, V> {
    async fn deserialize<R: AsyncRead + Unpin>(
        reader: &mut R,
        version: ProtocolVersion,
    ) -> Result<Self, ProtocolError> {
        let len = u64::deserialize(reader, version)
            .await
            .io_context("Failed to read BTreeMap length")?;

        if len > MAX_STRING_LIST_SIZE {
            return Err(ProtocolError::StringListTooLong {
                length: len,
                max: MAX_STRING_LIST_SIZE,
            });
        }

        let mut result = BTreeMap::new();
        for i in 0..len {
            let key = K::deserialize(reader, version)
                .await
                .io_context(format!("Failed to read BTreeMap key {i}"))?;
            let value = V::deserialize(reader, version)
                .await
                .io_context(format!("Failed to read BTreeMap value {i}"))?;
            result.insert(key, value);
        }

        Ok(result)
    }
}
