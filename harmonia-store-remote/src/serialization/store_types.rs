use crate::error::{IoErrorContext, ProtocolError};
use crate::protocol::{
    ProtocolVersion,
    types::{DerivedPath, Missing, OutputsSpec},
};
use crate::serialization::{Deserialize, Serialize};
use harmonia_store_core::{ContentAddress, FileIngestionMethod, NarSignature, StorePath};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};

// Serialization for NarSignature (formatted as "keyname:base64signature")
impl Serialize for NarSignature {
    async fn serialize<W: AsyncWrite + Unpin>(
        &self,
        writer: &mut W,
        version: ProtocolVersion,
    ) -> Result<(), ProtocolError> {
        // Calculate total length
        let len = self.key_name.len() + 1 + self.sig.to_base64().len();

        // Write length
        (len as u64).serialize(writer, version).await?;

        // Write data directly
        writer
            .write_all(&self.key_name)
            .await
            .io_context("Failed to write key name")?;
        writer
            .write_all(b":")
            .await
            .io_context("Failed to write colon")?;
        writer
            .write_all(self.sig.to_base64().as_bytes())
            .await
            .io_context("Failed to write signature")?;

        // Write padding
        let padding = ((len + 7) & !7) - len;
        if padding > 0 {
            writer
                .write_all(&vec![0u8; padding])
                .await
                .io_context("Failed to write padding")?;
        }

        Ok(())
    }
}

impl Deserialize for NarSignature {
    async fn deserialize<R: AsyncRead + Unpin>(
        reader: &mut R,
        version: ProtocolVersion,
    ) -> Result<Self, ProtocolError> {
        let bytes = Vec::<u8>::deserialize(reader, version)
            .await
            .io_context("Failed to read NarSignature")?;

        NarSignature::parse(&bytes).map_err(|e| ProtocolError::DaemonError {
            message: format!("Failed to parse NarSignature: {e}"),
        })
    }
}

// Serialization for ContentAddress
impl Serialize for ContentAddress {
    async fn serialize<W: AsyncWrite + Unpin>(
        &self,
        writer: &mut W,
        version: ProtocolVersion,
    ) -> Result<(), ProtocolError> {
        // Calculate length first
        let len = match self {
            ContentAddress::Text { hash } => {
                5 + 1 + hash.algo.name().len() + 1 + hash.to_hex().len() // "text:algo:hex"
            }
            ContentAddress::Fixed { method, hash } => {
                6 + (if matches!(method, FileIngestionMethod::Recursive) {
                    2
                } else {
                    0
                }) + 1
                    + hash.algo.name().len()
                    + 1
                    + hash.to_hex().len() // "fixed:[r:]algo:hex"
            }
        };

        // Write length
        (len as u64).serialize(writer, version).await?;

        // Write data
        match self {
            ContentAddress::Text { hash } => {
                writer
                    .write_all(b"text:")
                    .await
                    .io_context("Failed to write text prefix")?;
                writer
                    .write_all(hash.algo.name().as_bytes())
                    .await
                    .io_context("Failed to write hash algo")?;
                writer
                    .write_all(b":")
                    .await
                    .io_context("Failed to write colon")?;
                writer
                    .write_all(&hash.to_hex())
                    .await
                    .io_context("Failed to write hash")?;
            }
            ContentAddress::Fixed { method, hash } => {
                writer
                    .write_all(b"fixed:")
                    .await
                    .io_context("Failed to write fixed prefix")?;
                if matches!(method, FileIngestionMethod::Recursive) {
                    writer
                        .write_all(b"r:")
                        .await
                        .io_context("Failed to write r:")?;
                }
                writer
                    .write_all(hash.algo.name().as_bytes())
                    .await
                    .io_context("Failed to write hash algo")?;
                writer
                    .write_all(b":")
                    .await
                    .io_context("Failed to write colon")?;
                writer
                    .write_all(&hash.to_hex())
                    .await
                    .io_context("Failed to write hash")?;
            }
        }

        // Write padding
        let padding = ((len + 7) & !7) - len;
        if padding > 0 {
            writer
                .write_all(&vec![0u8; padding])
                .await
                .io_context("Failed to write padding")?;
        }

        Ok(())
    }
}

impl Deserialize for ContentAddress {
    async fn deserialize<R: AsyncRead + Unpin>(
        reader: &mut R,
        version: ProtocolVersion,
    ) -> Result<Self, ProtocolError> {
        let bytes = Vec::<u8>::deserialize(reader, version)
            .await
            .io_context("Failed to read ContentAddress")?;

        ContentAddress::parse(&bytes).map_err(|e| ProtocolError::DaemonError {
            message: format!("Failed to parse ContentAddress: {e}"),
        })
    }
}

// Serialization for DerivedPath
impl Serialize for DerivedPath {
    async fn serialize<W: AsyncWrite + Unpin>(
        &self,
        writer: &mut W,
        version: ProtocolVersion,
    ) -> Result<(), ProtocolError> {
        // Calculate total length first
        let len = match self {
            DerivedPath::Opaque(path) => path.as_bytes().len(),
            DerivedPath::Built(path, outputs) => {
                path.as_bytes().len()
                    + 1
                    + match outputs {
                        OutputsSpec::All => 1,
                        OutputsSpec::Names(names) => {
                            names.iter().map(|n| n.as_bytes().len()).sum::<usize>()
                                + names.len().saturating_sub(1) // commas
                        }
                    }
            }
        };

        // Write length
        (len as u64).serialize(writer, version).await?;

        // Write data
        match self {
            DerivedPath::Opaque(path) => {
                writer
                    .write_all(path.as_bytes())
                    .await
                    .io_context("Failed to write DerivedPath")?;
            }
            DerivedPath::Built(path, outputs) => {
                writer
                    .write_all(path.as_bytes())
                    .await
                    .io_context("Failed to write DerivedPath")?;
                writer
                    .write_all(b"!")
                    .await
                    .io_context("Failed to write DerivedPath separator")?;
                match outputs {
                    OutputsSpec::All => {
                        writer
                            .write_all(b"*")
                            .await
                            .io_context("Failed to write DerivedPath outputs")?;
                    }
                    OutputsSpec::Names(names) => {
                        for (i, name) in names.iter().enumerate() {
                            if i > 0 {
                                writer
                                    .write_all(b",")
                                    .await
                                    .io_context("Failed to write comma")?;
                            }
                            writer
                                .write_all(name.as_bytes())
                                .await
                                .io_context("Failed to write output name")?;
                        }
                    }
                }
            }
        }

        // Write padding
        let padding = ((len + 7) & !7) - len;
        if padding > 0 {
            writer
                .write_all(&vec![0u8; padding])
                .await
                .io_context("Failed to write padding")?;
        }

        Ok(())
    }
}

impl Deserialize for DerivedPath {
    async fn deserialize<R: AsyncRead + Unpin>(
        reader: &mut R,
        version: ProtocolVersion,
    ) -> Result<Self, ProtocolError> {
        let bytes = Vec::<u8>::deserialize(reader, version)
            .await
            .io_context("Failed to read DerivedPath")?;

        DerivedPath::parse(&bytes).map_err(|e| ProtocolError::DaemonError {
            message: format!("Failed to parse DerivedPath: {e}"),
        })
    }
}

// Serialization for Missing
impl Serialize for Missing {
    async fn serialize<W: AsyncWrite + Unpin>(
        &self,
        writer: &mut W,
        version: ProtocolVersion,
    ) -> Result<(), ProtocolError> {
        // Protocol expects these fields in this specific order
        // BTreeSet serializes the same as Vec (length + items)
        self.will_build.serialize(writer, version).await?;
        self.will_substitute.serialize(writer, version).await?;
        self.unknown_paths.serialize(writer, version).await?;
        self.download_size.serialize(writer, version).await?;
        self.nar_size.serialize(writer, version).await?;
        Ok(())
    }
}

impl Deserialize for Missing {
    async fn deserialize<R: AsyncRead + Unpin>(
        reader: &mut R,
        version: ProtocolVersion,
    ) -> Result<Self, ProtocolError> {
        let will_build_vec = Vec::<StorePath>::deserialize(reader, version)
            .await
            .io_context("Failed to read will_build")?;
        let will_substitute_vec = Vec::<StorePath>::deserialize(reader, version)
            .await
            .io_context("Failed to read will_substitute")?;
        let unknown_paths_vec = Vec::<StorePath>::deserialize(reader, version)
            .await
            .io_context("Failed to read unknown_paths")?;
        let download_size = u64::deserialize(reader, version)
            .await
            .io_context("Failed to read download_size")?;
        let nar_size = u64::deserialize(reader, version)
            .await
            .io_context("Failed to read nar_size")?;

        Ok(Missing {
            will_build: will_build_vec.into_iter().collect(),
            will_substitute: will_substitute_vec.into_iter().collect(),
            unknown_paths: unknown_paths_vec.into_iter().collect(),
            download_size,
            nar_size,
        })
    }
}
