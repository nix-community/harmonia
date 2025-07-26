use crate::error::{IoErrorContext, ProtocolError};
use crate::protocol::{OpCode, ProtocolVersion, StorePath};
use crate::protocol::{
    CURRENT_PROTOCOL_VERSION, MIN_PROTOCOL_VERSION, WORKER_MAGIC_1, WORKER_MAGIC_2,
};
use crate::serialization::{Deserialize, Serialize};
use crate::server::RequestHandler;
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

                let result = handler.handle_query_path_info(&path).await?;
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

                let result = handler.handle_is_valid_path(&path).await?;
                result.serialize(&mut stream, version).await?;
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
        // Send server features
        Vec::<Vec<u8>>::new()
            .serialize(stream, client_version)
            .await?;
        // Read client features
        let _client_features = Vec::<Vec<u8>>::deserialize(stream, client_version).await?;
    }

    // Send daemon version string
    b"harmonia-store-remote 0.1.0"
        .to_vec()
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
