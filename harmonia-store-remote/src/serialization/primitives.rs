use crate::error::ProtocolError;
use crate::protocol::{ProtocolVersion, MAX_STRING_LIST_SIZE, MAX_STRING_SIZE};
use crate::serialization::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

impl Serialize for u64 {
    async fn serialize<W: AsyncWrite + Unpin>(
        &self,
        writer: &mut W,
        _version: ProtocolVersion,
    ) -> Result<(), ProtocolError> {
        writer.write_all(&self.to_le_bytes()).await?;
        Ok(())
    }
}

impl Deserialize for u64 {
    async fn deserialize<R: AsyncRead + Unpin>(
        reader: &mut R,
        _version: ProtocolVersion,
    ) -> Result<Self, ProtocolError> {
        let mut buf = [0; 8];
        reader.read_exact(&mut buf).await?;
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
        Ok(u64::deserialize(reader, version).await? != 0)
    }
}

impl Serialize for String {
    async fn serialize<W: AsyncWrite + Unpin>(
        &self,
        writer: &mut W,
        version: ProtocolVersion,
    ) -> Result<(), ProtocolError> {
        let len = self.len() as u64;
        len.serialize(writer, version).await?;
        writer.write_all(self.as_bytes()).await?;

        // Padding to 8-byte boundary
        let padding_size = (8 - len % 8) % 8;
        if padding_size > 0 {
            let padding = [0u8; 8];
            writer.write_all(&padding[..padding_size as usize]).await?;
        }
        Ok(())
    }
}

impl Deserialize for String {
    async fn deserialize<R: AsyncRead + Unpin>(
        reader: &mut R,
        version: ProtocolVersion,
    ) -> Result<Self, ProtocolError> {
        let len = u64::deserialize(reader, version).await?;

        if len > MAX_STRING_SIZE {
            return Err(ProtocolError::StringTooLong {
                length: len,
                max: MAX_STRING_SIZE,
            });
        }

        // Align to the next multiple of 8
        let aligned_len = (len + 7) & !7;
        let mut buf = vec![0; aligned_len as usize];
        reader.read_exact(&mut buf).await?;

        Ok(std::str::from_utf8(&buf[..len as usize])?.to_owned())
    }
}

impl<T: Serialize> Serialize for Vec<T> {
    async fn serialize<W: AsyncWrite + Unpin>(
        &self,
        writer: &mut W,
        version: ProtocolVersion,
    ) -> Result<(), ProtocolError> {
        (self.len() as u64).serialize(writer, version).await?;
        for item in self {
            item.serialize(writer, version).await?;
        }
        Ok(())
    }
}

impl Deserialize for Vec<String> {
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
            result.push(String::deserialize(reader, version).await?);
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
            None => 0u64.serialize(writer, version).await,
            Some(value) => {
                1u64.serialize(writer, version).await?;
                value.serialize(writer, version).await
            }
        }
    }
}

impl<T: Deserialize> Deserialize for Option<T> {
    async fn deserialize<R: AsyncRead + Unpin>(
        reader: &mut R,
        version: ProtocolVersion,
    ) -> Result<Self, ProtocolError> {
        let present = u64::deserialize(reader, version).await?;
        if present == 0 {
            Ok(None)
        } else {
            Ok(Some(T::deserialize(reader, version).await?))
        }
    }
}
