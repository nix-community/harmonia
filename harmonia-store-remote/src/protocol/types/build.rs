use crate::error::ProtocolError;
use crate::protocol::ProtocolVersion;
use crate::serialization::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncWrite};

// Re-export build types from core
pub use harmonia_store_core::{BuildMode, BuildResult, BuildStatus, DrvOutputResult};

impl Serialize for BuildMode {
    async fn serialize<W: AsyncWrite + Unpin>(
        &self,
        writer: &mut W,
        version: ProtocolVersion,
    ) -> Result<(), ProtocolError> {
        (*self as u64).serialize(writer, version).await
    }
}

impl Deserialize for BuildMode {
    async fn deserialize<R: AsyncRead + Unpin>(
        reader: &mut R,
        version: ProtocolVersion,
    ) -> Result<Self, ProtocolError> {
        let val = u64::deserialize(reader, version).await?;
        match val {
            0 => Ok(BuildMode::Normal),
            1 => Ok(BuildMode::Repair),
            2 => Ok(BuildMode::Check),
            _ => Err(ProtocolError::DaemonError {
                message: format!("Invalid BuildMode value: {val}"),
            }),
        }
    }
}

impl Deserialize for BuildStatus {
    async fn deserialize<R: AsyncRead + Unpin>(
        reader: &mut R,
        version: ProtocolVersion,
    ) -> Result<Self, ProtocolError> {
        let val = u64::deserialize(reader, version).await?;
        match val {
            0 => Ok(BuildStatus::Built),
            1 => Ok(BuildStatus::Substituted),
            2 => Ok(BuildStatus::AlreadyValid),
            3 => Ok(BuildStatus::PermanentFailure),
            4 => Ok(BuildStatus::InputRejected),
            5 => Ok(BuildStatus::OutputRejected),
            6 => Ok(BuildStatus::TransientFailure),
            7 => Ok(BuildStatus::TimedOut),
            8 => Ok(BuildStatus::MiscFailure),
            9 => Ok(BuildStatus::DependencyFailed),
            10 => Ok(BuildStatus::LogLimitExceeded),
            11 => Ok(BuildStatus::NotDeterministic),
            12 => Ok(BuildStatus::ResolvesToAlreadyValid),
            13 => Ok(BuildStatus::NoSubstituters),
            _ => Err(ProtocolError::DaemonError {
                message: format!("Invalid BuildStatus value: {val}"),
            }),
        }
    }
}

impl Deserialize for BuildResult {
    async fn deserialize<R: AsyncRead + Unpin>(
        reader: &mut R,
        version: ProtocolVersion,
    ) -> Result<Self, ProtocolError> {
        use harmonia_store_core::StorePath;
        use std::collections::BTreeMap;

        let status = BuildStatus::deserialize(reader, version).await?;

        let error_msg = {
            let msg = Vec::<u8>::deserialize(reader, version).await?;
            if msg.is_empty() { None } else { Some(msg) }
        };

        // For protocol < 29, read log lines (obsolete)
        let log_lines = if version.minor < 29 {
            // Number of log lines as u64, then each line
            let num_lines = u64::deserialize(reader, version).await?;
            let mut lines = Vec::new();
            for _ in 0..num_lines {
                lines.push(Vec::<u8>::deserialize(reader, version).await?);
            }
            lines
        } else {
            Vec::new()
        };

        let times_built = if version.minor >= 26 {
            u64::deserialize(reader, version).await? as u32
        } else {
            0
        };

        let is_non_deterministic = if version.minor >= 26 {
            bool::deserialize(reader, version).await?
        } else {
            false
        };

        let start_time = if version.minor >= 26 {
            u64::deserialize(reader, version).await?
        } else {
            0
        };

        let stop_time = if version.minor >= 26 {
            u64::deserialize(reader, version).await?
        } else {
            0
        };

        let built_outputs = if version.minor >= 28 {
            // First, read DrvOutputs map (output name -> output id)
            let drv_outputs = BTreeMap::<Vec<u8>, Vec<u8>>::deserialize(reader, version).await?;

            // Then read realizations (output id -> store path)
            let num_realizations = u64::deserialize(reader, version).await?;
            let mut realizations = BTreeMap::new();

            for _ in 0..num_realizations {
                // Read output id
                let output_id = Vec::<u8>::deserialize(reader, version).await?;
                // Read store path
                let path = StorePath::deserialize(reader, version).await?;
                // For now we ignore the hash field
                let _hash_bytes = Vec::<u8>::deserialize(reader, version).await?;

                realizations.insert(
                    output_id,
                    DrvOutputResult {
                        path,
                        hash: None, // TODO: Parse hash from bytes
                    },
                );
            }

            // Combine drv_outputs with realizations to get output name -> result mapping
            let mut built_outputs = BTreeMap::new();
            for (name, id) in drv_outputs {
                if let Some(result) = realizations.get(&id) {
                    built_outputs.insert(name, result.clone());
                }
            }
            built_outputs
        } else {
            BTreeMap::new()
        };

        Ok(BuildResult {
            status,
            error_msg,
            log_lines,
            times_built,
            is_non_deterministic,
            start_time,
            stop_time,
            built_outputs,
        })
    }
}

/// Status of a single derivation output build result
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DrvOutputStatus {
    /// Output was built successfully
    Built,
    /// Output was substituted from a binary cache
    Substituted,
    /// Output already existed and was valid
    AlreadyValid,
}
