pub mod connection;

use crate::error::ProtocolError;
use crate::protocol::{OpCode, ProtocolVersion, StorePath, ValidPathInfo};
use crate::serialization::{Deserialize, Serialize};
use connection::Connection;
use std::path::Path;

#[derive(Debug)]
pub struct DaemonClient {
    connection: Connection,
    version: ProtocolVersion,
    #[allow(dead_code)]
    features: Vec<String>,
}

impl DaemonClient {
    pub async fn connect(path: &Path) -> Result<Self, ProtocolError> {
        let (connection, version, features) = Connection::connect(path).await?;
        Ok(Self {
            connection,
            version,
            features,
        })
    }

    pub async fn query_path_info(
        &mut self,
        path: &StorePath,
    ) -> Result<Option<ValidPathInfo>, ProtocolError> {
        self.execute_operation(OpCode::QueryPathInfo, path).await
    }

    pub async fn query_path_from_hash_part(
        &mut self,
        hash: &str,
    ) -> Result<Option<StorePath>, ProtocolError> {
        // Special case: Nix uses empty string for None
        let response: String = self
            .execute_operation(OpCode::QueryPathFromHashPart, &hash.to_string())
            .await?;
        Ok(if response.is_empty() {
            None
        } else {
            Some(StorePath::new(response))
        })
    }

    pub async fn is_valid_path(&mut self, path: &StorePath) -> Result<bool, ProtocolError> {
        self.execute_operation(OpCode::IsValidPath, path).await
    }

    async fn execute_operation<Req: Serialize, Resp: Deserialize>(
        &mut self,
        opcode: OpCode,
        request: &Req,
    ) -> Result<Resp, ProtocolError> {
        self.connection.send_opcode(opcode).await?;
        request
            .serialize(&mut self.connection, self.version)
            .await?;
        self.connection.process_stderr().await?;
        Resp::deserialize(&mut self.connection, self.version).await
    }
}
