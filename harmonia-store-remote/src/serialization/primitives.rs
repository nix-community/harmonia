use crate::error::{IoErrorContext, ProtocolError};
use crate::protocol::{MAX_STRING_LIST_SIZE, MAX_STRING_SIZE, ProtocolVersion};
use crate::serialization::{Deserialize, Serialize};
use std::collections::BTreeSet;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

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

impl Deserialize for String {
    async fn deserialize<R: AsyncRead + Unpin>(
        reader: &mut R,
        version: ProtocolVersion,
    ) -> Result<Self, ProtocolError> {
        let len = u64::deserialize(reader, version)
            .await
            .io_context("Failed to read string length")?;

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
            .io_context("Failed to read string data")?;

        Ok(std::str::from_utf8(&buf[..len as usize])?.to_owned())
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

impl<T: Serialize> Serialize for Vec<T> {
    async fn serialize<W: AsyncWrite + Unpin>(
        &self,
        writer: &mut W,
        version: ProtocolVersion,
    ) -> Result<(), ProtocolError> {
        (self.len() as u64)
            .serialize(writer, version)
            .await
            .io_context("Failed to write Vec length")?;
        for (i, item) in self.iter().enumerate() {
            item.serialize(writer, version)
                .await
                .io_context(format!("Failed to write Vec item {i}"))?;
        }
        Ok(())
    }
}

impl Deserialize for Vec<Vec<u8>> {
    async fn deserialize<R: AsyncRead + Unpin>(
        reader: &mut R,
        version: ProtocolVersion,
    ) -> Result<Self, ProtocolError> {
        let len = u64::deserialize(reader, version)
            .await
            .io_context("Failed to read Vec<Vec<u8>> length")?;

        if len > MAX_STRING_LIST_SIZE {
            return Err(ProtocolError::StringListTooLong {
                length: len,
                max: MAX_STRING_LIST_SIZE,
            });
        }

        let mut result = Vec::with_capacity(len as usize);
        for i in 0..len {
            result.push(
                Vec::<u8>::deserialize(reader, version)
                    .await
                    .io_context(format!("Failed to read Vec<Vec<u8>> item {i}"))?,
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
        (self.len() as u64)
            .serialize(writer, version)
            .await
            .io_context("Failed to write BTreeSet length")?;
        for (i, item) in self.iter().enumerate() {
            item.serialize(writer, version)
                .await
                .io_context(format!("Failed to write BTreeSet item {i}"))?;
        }
        Ok(())
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
