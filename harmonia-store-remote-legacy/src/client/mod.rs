pub mod connection;
pub mod metrics;
pub mod pool;

use crate::error::ProtocolError;
use crate::protocol::{OpCode, ValidPathInfo};
use crate::serialization::{Deserialize, Serialize};
use harmonia_store_core::store_path::StorePath;
use pool::ConnectionPool;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

// Re-export pool types for public use
pub use metrics::ClientMetrics;
pub use pool::PoolConfig;

#[derive(Clone)]
pub struct DaemonClient {
    pool: Arc<ConnectionPool>,
    store_dir: harmonia_store_core::store_path::StoreDir,
}

impl DaemonClient {
    /// Create a new DaemonClient with default pool configuration
    pub async fn connect(path: &Path) -> Result<Self, ProtocolError> {
        Self::connect_with_config(path, PoolConfig::default()).await
    }

    /// Create a new DaemonClient with custom configuration
    pub async fn connect_with_config(
        path: &Path,
        pool_config: PoolConfig,
    ) -> Result<Self, ProtocolError> {
        // Use default store directory ("/nix/store")
        let store_dir = harmonia_store_core::store_path::StoreDir::default();
        let pool = Arc::new(ConnectionPool::new(path.to_path_buf(), store_dir.clone(), pool_config));
        Ok(Self { pool, store_dir })
    }

    pub async fn query_path_info(
        &self,
        path: &StorePath,
    ) -> Result<Option<ValidPathInfo>, ProtocolError> {
        self.execute_operation(OpCode::QueryPathInfo, path).await
    }

    pub async fn query_path_from_hash_part(
        &self,
        hash: &[u8],
    ) -> Result<Option<StorePath>, ProtocolError> {
        // Special case: Nix uses empty string for None
        let response: Vec<u8> = self
            .execute_operation(OpCode::QueryPathFromHashPart, &hash)
            .await?;

        Ok(if response.is_empty() {
            None
        } else {
            let path_str = std::str::from_utf8(&response).map_err(|e| ProtocolError::DaemonError {
                message: format!("StorePath is not valid UTF-8: {e}"),
            })?;
            Some(self.store_dir.parse(path_str).map_err(|e| ProtocolError::DaemonError {
                message: format!("Failed to parse StorePath: {e}"),
            })?)
        })
    }

    pub async fn is_valid_path(&self, path: &StorePath) -> Result<bool, ProtocolError> {
        self.execute_operation(OpCode::IsValidPath, path).await
    }

    async fn execute_operation<Req: Serialize, Resp: Deserialize>(
        &self,
        opcode: OpCode,
        request: &Req,
    ) -> Result<Resp, ProtocolError> {
        // Retry configuration
        const MAX_ATTEMPTS: u32 = 3;

        let mut last_error = None;

        for attempt in 0..MAX_ATTEMPTS {
            let mut guard = match self.pool.acquire().await {
                Ok(guard) => guard,
                Err(e) => {
                    last_error = Some(e);
                    if attempt < MAX_ATTEMPTS - 1 {
                        tokio::time::sleep(Self::calculate_delay(attempt)).await;
                        continue;
                    }
                    break;
                }
            };

            // Try to execute the operation
            let result = async {
                let (conn, version) = guard.connection_and_version();

                conn.send_opcode(opcode).await?;
                request.serialize(conn, version, &self.store_dir).await?;
                conn.process_stderr().await?;
                Resp::deserialize(conn, version, &self.store_dir).await
            }
            .await;

            match result {
                Ok(response) => return Ok(response),
                Err(e) => {
                    // Determine if error is retryable
                    let should_retry = matches!(
                        &e,
                        ProtocolError::Io { .. }
                            | ProtocolError::ConnectionTimeout
                            | ProtocolError::PoolTimeout
                    );

                    if should_retry {
                        guard.mark_broken();
                    }

                    last_error = Some(e);

                    // If not retryable or last attempt, return error
                    if !should_retry || attempt == MAX_ATTEMPTS - 1 {
                        break;
                    }

                    // Wait before retrying
                    tokio::time::sleep(Self::calculate_delay(attempt)).await;
                }
            }
        }

        Err(last_error.unwrap())
    }

    fn calculate_delay(attempt: u32) -> Duration {
        const INITIAL_DELAY: Duration = Duration::from_millis(100);
        const MAX_DELAY: Duration = Duration::from_secs(5);
        const BACKOFF_MULTIPLIER: f64 = 2.0;

        let mut delay = INITIAL_DELAY.as_secs_f64() * BACKOFF_MULTIPLIER.powi(attempt as i32);
        delay = delay.min(MAX_DELAY.as_secs_f64());

        // Add simple jitter based on current time
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as f64;
        let jitter = (seed % 20.0) / 100.0; // 0-20% jitter
        delay *= 1.0 + jitter;

        Duration::from_secs_f64(delay)
    }
}
