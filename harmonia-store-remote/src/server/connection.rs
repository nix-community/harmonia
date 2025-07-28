use crate::error::{IoErrorContext, ProtocolError};
use crate::protocol::types::DrvOutputId;
use crate::protocol::{
    CURRENT_PROTOCOL_VERSION, MIN_PROTOCOL_VERSION, WORKER_MAGIC_1, WORKER_MAGIC_2,
};
use crate::protocol::{OpCode, ProtocolVersion, StorePath};
use crate::serialization::{Deserialize, Serialize};
use crate::server::RequestHandler;
use std::collections::BTreeSet;
use tokio::net::UnixStream;

pub async fn handle_connection<H: RequestHandler>(
    mut stream: UnixStream,
    handler: H,
) -> Result<(), ProtocolError> {
    // Perform handshake
    let version = handshake(&mut stream).await?;

    // Main request loop
    loop {
        // Read opcode
        let opcode_raw = match u64::deserialize(&mut stream, version).await {
            Ok(op) => op,
            Err(_) => break, // Connection closed
        };

        let opcode = OpCode::try_from(opcode_raw)?;

        // Handle operation
        match opcode {
            OpCode::QueryPathInfo => {
                let path = StorePath::deserialize(&mut stream, version).await?;
                send_stderr_last(&mut stream, version).await?;

                let result = handler.handle_query_path_info(path).await?;
                result.serialize(&mut stream, version).await?;
            }

            OpCode::QueryPathFromHashPart => {
                let hash = <Vec<u8>>::deserialize(&mut stream, version).await?;
                send_stderr_last(&mut stream, version).await?;

                let result = handler.handle_query_path_from_hash_part(&hash).await?;
                // Nix protocol uses empty string for None
                match result {
                    Some(path) => path.as_bytes().serialize(&mut stream, version).await?,
                    None => (&[] as &[u8]).serialize(&mut stream, version).await?,
                }
            }

            OpCode::IsValidPath => {
                let path = StorePath::deserialize(&mut stream, version).await?;
                send_stderr_last(&mut stream, version).await?;

                let result = handler.handle_is_valid_path(path).await?;
                result.serialize(&mut stream, version).await?;
            }

            OpCode::QueryAllValidPaths => {
                send_stderr_last(&mut stream, version).await?;

                let result = handler.handle_query_all_valid_paths().await?;
                result.serialize(&mut stream, version).await?;
            }

            OpCode::QueryValidPaths => {
                let paths = BTreeSet::<StorePath>::deserialize(&mut stream, version).await?;
                send_stderr_last(&mut stream, version).await?;

                let result = handler.handle_query_valid_paths(paths).await?;
                result.serialize(&mut stream, version).await?;
            }

            OpCode::QuerySubstitutablePaths => {
                let paths = BTreeSet::<StorePath>::deserialize(&mut stream, version).await?;
                send_stderr_last(&mut stream, version).await?;

                let result = handler.handle_query_substitutable_paths(paths).await?;
                result.serialize(&mut stream, version).await?;
            }

            OpCode::HasSubstitutes => {
                let path = StorePath::deserialize(&mut stream, version).await?;
                send_stderr_last(&mut stream, version).await?;

                let result = handler.handle_has_substitutes(path).await?;
                result.serialize(&mut stream, version).await?;
            }

            OpCode::QuerySubstitutablePathInfo => {
                let path = StorePath::deserialize(&mut stream, version).await?;
                send_stderr_last(&mut stream, version).await?;

                let result = handler.handle_query_substitutable_path_info(path).await?;
                // Use same serialization as ValidPathInfo for Option
                result.serialize(&mut stream, version).await?;
            }

            OpCode::QuerySubstitutablePathInfos => {
                let paths = BTreeSet::<StorePath>::deserialize(&mut stream, version).await?;
                send_stderr_last(&mut stream, version).await?;

                let result = handler.handle_query_substitutable_path_infos(paths).await?;
                result.serialize(&mut stream, version).await?;
            }

            OpCode::QueryReferrers => {
                let path = StorePath::deserialize(&mut stream, version).await?;
                send_stderr_last(&mut stream, version).await?;

                let result = handler.handle_query_referrers(path).await?;
                result.serialize(&mut stream, version).await?;
            }

            OpCode::QueryValidDerivers => {
                let path = StorePath::deserialize(&mut stream, version).await?;
                send_stderr_last(&mut stream, version).await?;

                let result = handler.handle_query_valid_derivers(path).await?;
                result.serialize(&mut stream, version).await?;
            }

            OpCode::QueryDerivationOutputs => {
                let drv_path = StorePath::deserialize(&mut stream, version).await?;
                send_stderr_last(&mut stream, version).await?;

                let result = handler.handle_query_derivation_outputs(drv_path).await?;
                result.serialize(&mut stream, version).await?;
            }

            OpCode::QueryDerivationOutputNames => {
                let drv_path = StorePath::deserialize(&mut stream, version).await?;
                send_stderr_last(&mut stream, version).await?;

                let result = handler
                    .handle_query_derivation_output_names(drv_path)
                    .await?;
                result.as_slice().serialize(&mut stream, version).await?;
            }

            OpCode::QueryDerivationOutputMap => {
                let drv_path = StorePath::deserialize(&mut stream, version).await?;
                send_stderr_last(&mut stream, version).await?;

                let result = handler.handle_query_derivation_output_map(drv_path).await?;
                result.serialize(&mut stream, version).await?;
            }

            OpCode::QueryMissing => {
                use crate::protocol::types::DerivedPath;
                let targets = Vec::<DerivedPath>::deserialize(&mut stream, version).await?;
                send_stderr_last(&mut stream, version).await?;

                let result = handler.handle_query_missing(targets).await?;
                result.serialize(&mut stream, version).await?;
            }

            OpCode::QueryRealisation => {
                let id = DrvOutputId::deserialize(&mut stream, version).await?;
                send_stderr_last(&mut stream, version).await?;

                let result = handler.handle_query_realisation(id).await?;
                result.serialize(&mut stream, version).await?;
            }

            OpCode::QueryFailedPaths => {
                send_stderr_last(&mut stream, version).await?;

                let result = handler.handle_query_failed_paths().await?;
                result.serialize(&mut stream, version).await?;
            }

            OpCode::ClearFailedPaths => {
                let paths = BTreeSet::<StorePath>::deserialize(&mut stream, version).await?;
                send_stderr_last(&mut stream, version).await?;

                handler.handle_clear_failed_paths(paths).await?;
                // No response needed
            }

            _ => {
                return Err(ProtocolError::InvalidOpCode(opcode_raw));
            }
        }
    }

    Ok(())
}

async fn handshake(stream: &mut UnixStream) -> Result<ProtocolVersion, ProtocolError> {
    // Read client magic
    let magic = u64::deserialize(stream, CURRENT_PROTOCOL_VERSION).await?;
    if magic != WORKER_MAGIC_1 {
        return Err(ProtocolError::InvalidMagic {
            expected: WORKER_MAGIC_1,
            actual: magic,
        });
    }

    // Send server magic
    WORKER_MAGIC_2
        .serialize(stream, CURRENT_PROTOCOL_VERSION)
        .await
        .io_context("Failed to write server magic number")?;

    // Send server version
    u64::from(CURRENT_PROTOCOL_VERSION)
        .serialize(stream, CURRENT_PROTOCOL_VERSION)
        .await
        .io_context("Failed to write server protocol version")?;

    // Read client version
    let client_version =
        ProtocolVersion::from(u64::deserialize(stream, CURRENT_PROTOCOL_VERSION).await?);

    if client_version < MIN_PROTOCOL_VERSION {
        return Err(ProtocolError::IncompatibleVersion {
            server: CURRENT_PROTOCOL_VERSION,
            min: MIN_PROTOCOL_VERSION,
            max: CURRENT_PROTOCOL_VERSION,
        });
    }

    // Read obsolete fields
    let _cpu_affinity = u64::deserialize(stream, client_version).await?;
    let _reserve_space = u64::deserialize(stream, client_version).await?;

    // Exchange features (if protocol >= 1.38)
    if client_version
        >= (ProtocolVersion {
            major: 1,
            minor: 38,
        })
    {
        // Send empty server features list
        0u64.serialize(stream, client_version).await?;
        // Read client features
        let _client_features = Vec::<Vec<u8>>::deserialize(stream, client_version).await?;
    }

    // Send daemon version string
    (b"harmonia-store-remote 0.1.0" as &[u8])
        .serialize(stream, client_version)
        .await?;

    // Send trust status (always trusted for now)
    true.serialize(stream, client_version).await?;

    // Send stderr Last message
    send_stderr_last(stream, client_version).await?;

    Ok(client_version)
}

async fn send_stderr_last(
    stream: &mut UnixStream,
    version: ProtocolVersion,
) -> Result<(), ProtocolError> {
    // Send STDERR_LAST message
    use crate::protocol::Msg;
    (Msg::Last as u64).serialize(stream, version).await
}
