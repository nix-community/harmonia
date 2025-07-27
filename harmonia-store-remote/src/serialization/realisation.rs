use crate::error::ProtocolError;
use crate::protocol::ProtocolVersion;
use crate::protocol::types::{DaemonSettings, DrvOutputId, Realisation};
use crate::serialization::{Deserialize as WireDeserialize, Serialize as WireSerialize};
use harmonia_store_core::{NarSignature, StorePath};
use serde::{Deserialize as SerdeDeserialize, Serialize as SerdeSerialize};
use serde_json;
use std::collections::BTreeMap;
use tokio::io::{AsyncRead, AsyncWrite};

impl WireSerialize for DrvOutputId {
    async fn serialize<W: AsyncWrite + Unpin>(
        &self,
        writer: &mut W,
        version: ProtocolVersion,
    ) -> Result<(), ProtocolError> {
        <&[u8] as WireSerialize>::serialize(&self.drv_hash.as_slice(), writer, version).await?;
        <&[u8] as WireSerialize>::serialize(&self.output_name.as_slice(), writer, version).await?;
        Ok(())
    }
}

impl WireDeserialize for DrvOutputId {
    async fn deserialize<R: AsyncRead + Unpin>(
        reader: &mut R,
        version: ProtocolVersion,
    ) -> Result<Self, ProtocolError> {
        Ok(DrvOutputId {
            drv_hash: <Vec<u8> as WireDeserialize>::deserialize(reader, version).await?,
            output_name: <Vec<u8> as WireDeserialize>::deserialize(reader, version).await?,
        })
    }
}

// JSON representation of Realisation for wire protocol
#[derive(SerdeSerialize, SerdeDeserialize)]
struct RealisationJson {
    id: String,
    #[serde(rename = "outPath")]
    out_path: String,
    signatures: Vec<String>,
    #[serde(rename = "dependentRealisations")]
    dependent_realisations: BTreeMap<String, String>,
}

impl WireSerialize for Realisation {
    async fn serialize<W: AsyncWrite + Unpin>(
        &self,
        writer: &mut W,
        version: ProtocolVersion,
    ) -> Result<(), ProtocolError> {
        // Convert to JSON representation
        let drv_hash =
            std::str::from_utf8(&self.id.drv_hash).map_err(|_| ProtocolError::DaemonError {
                message: "Invalid UTF-8 in drv_hash".to_string(),
            })?;
        let output_name =
            std::str::from_utf8(&self.id.output_name).map_err(|_| ProtocolError::DaemonError {
                message: "Invalid UTF-8 in output_name".to_string(),
            })?;
        let id_str = format!("{drv_hash}!{output_name}");

        let json_repr = RealisationJson {
            id: id_str,
            out_path: self.out_path.to_string(),
            signatures: self.signatures.iter().map(|s| s.to_string()).collect(),
            dependent_realisations: self
                .dependent_realisations
                .iter()
                .map(|(k, v)| {
                    let k_hash = std::str::from_utf8(&k.drv_hash).unwrap_or("<invalid>");
                    let k_output = std::str::from_utf8(&k.output_name).unwrap_or("<invalid>");
                    (format!("{k_hash}!{k_output}"), v.to_string())
                })
                .collect(),
        };

        let json_str =
            serde_json::to_string(&json_repr).map_err(|e| ProtocolError::DaemonError {
                message: format!("Failed to serialize Realisation to JSON: {e}"),
            })?;

        // Send as a string over the wire
        <&[u8] as WireSerialize>::serialize(&json_str.as_bytes(), writer, version).await
    }
}

impl WireDeserialize for Realisation {
    async fn deserialize<R: AsyncRead + Unpin>(
        reader: &mut R,
        version: ProtocolVersion,
    ) -> Result<Self, ProtocolError> {
        // Read JSON string from wire
        let json_bytes = <Vec<u8> as WireDeserialize>::deserialize(reader, version).await?;
        let json_str =
            std::str::from_utf8(&json_bytes).map_err(|_| ProtocolError::DaemonError {
                message: "Invalid UTF-8 in Realisation JSON".to_string(),
            })?;

        let json_repr: RealisationJson =
            serde_json::from_str(json_str).map_err(|e| ProtocolError::DaemonError {
                message: format!("Failed to parse Realisation JSON: {e}"),
            })?;

        // Parse the ID (format: "hash!outputName")
        let id_parts: Vec<&str> = json_repr.id.split('!').collect();
        if id_parts.len() != 2 {
            return Err(ProtocolError::DaemonError {
                message: format!("Invalid Realisation ID format: {}", json_repr.id),
            });
        }

        let id = DrvOutputId {
            drv_hash: id_parts[0].as_bytes().to_vec(),
            output_name: id_parts[1].as_bytes().to_vec(),
        };

        // Parse signatures
        let signatures = json_repr
            .signatures
            .iter()
            .filter_map(|s| NarSignature::parse(s.as_bytes()).ok())
            .collect();

        // Parse dependent realisations
        let dependent_realisations = json_repr
            .dependent_realisations
            .into_iter()
            .filter_map(|(k, v)| {
                let k_parts: Vec<&str> = k.split('!').collect();
                if k_parts.len() == 2 {
                    Some((
                        DrvOutputId {
                            drv_hash: k_parts[0].as_bytes().to_vec(),
                            output_name: k_parts[1].as_bytes().to_vec(),
                        },
                        StorePath::from(v.into_bytes()),
                    ))
                } else {
                    None
                }
            })
            .collect();

        Ok(Realisation {
            id,
            out_path: StorePath::from(json_repr.out_path.into_bytes()),
            signatures,
            dependent_realisations,
        })
    }
}

// Note: DaemonSettings is only used for sending, not receiving
impl WireSerialize for DaemonSettings {
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