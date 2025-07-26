pub mod connection;
pub mod metrics;
pub mod pool;

use crate::error::ProtocolError;
use crate::framed::FramedSink;
use crate::protocol::{
    OpCode, ValidPathInfo,
    types::{AddSignaturesRequest, AddTextToStoreRequest, DerivedPath, Missing},
};
use crate::serialization::{Deserialize, Serialize};
use harmonia_store_core::{FileIngestionMethod, HashAlgo, NarSignature, StorePath};
use pool::ConnectionPool;
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};

// Re-export pool types for public use
pub use metrics::ClientMetrics;
pub use pool::PoolConfig;

#[derive(Clone)]
pub struct DaemonClient {
    pool: Arc<ConnectionPool>,
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
        let pool = Arc::new(ConnectionPool::new(path.to_path_buf(), pool_config));
        Ok(Self { pool })
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
            Some(StorePath::new(response))
        })
    }

    pub async fn is_valid_path(&self, path: &StorePath) -> Result<bool, ProtocolError> {
        self.execute_operation(OpCode::IsValidPath, path).await
    }

    pub async fn query_all_valid_paths(&self) -> Result<Vec<StorePath>, ProtocolError> {
        self.execute_operation(OpCode::QueryAllValidPaths, &())
            .await
    }

    pub async fn query_valid_paths(&self, paths: &[StorePath]) -> Result<Vec<bool>, ProtocolError> {
        self.execute_operation(OpCode::QueryValidPaths, &paths)
            .await
    }

    pub async fn query_missing(&self, paths: &[DerivedPath]) -> Result<Missing, ProtocolError> {
        self.execute_operation(OpCode::QueryMissing, &paths).await
    }

    pub async fn query_referrers(
        &self,
        path: &StorePath,
    ) -> Result<BTreeSet<StorePath>, ProtocolError> {
        self.execute_operation(OpCode::QueryReferrers, path).await
    }

    pub async fn query_valid_derivers(
        &self,
        path: &StorePath,
    ) -> Result<Vec<StorePath>, ProtocolError> {
        self.execute_operation(OpCode::QueryValidDerivers, path)
            .await
    }

    pub async fn query_substitutable_paths(
        &self,
        paths: &[StorePath],
    ) -> Result<BTreeSet<StorePath>, ProtocolError> {
        self.execute_operation(OpCode::QuerySubstitutablePaths, &paths)
            .await
    }

    pub async fn query_derivation_outputs(
        &self,
        drv: &StorePath,
    ) -> Result<Vec<StorePath>, ProtocolError> {
        self.execute_operation(OpCode::QueryDerivationOutputs, drv)
            .await
    }

    pub async fn query_derivation_output_names(
        &self,
        drv: &StorePath,
    ) -> Result<Vec<Vec<u8>>, ProtocolError> {
        self.execute_operation(OpCode::QueryDerivationOutputNames, drv)
            .await
    }

    // Store Modification Operations

    pub async fn add_to_store<R: AsyncRead + Unpin>(
        &self,
        name: &[u8],
        mut source: R,
        method: FileIngestionMethod,
        hash_algo: HashAlgo,
        references: &BTreeSet<StorePath>,
        repair: bool,
    ) -> Result<StorePath, ProtocolError> {
        // For protocol >= 25, we send content address method string
        // For older protocols, we send fixed/recursive flags

        let mut guard = self.pool.acquire().await?;
        let (conn, version) = guard.connection_and_version();

        // Send operation code
        conn.send_opcode(OpCode::AddToStore).await?;

        if version.minor >= 25 {
            // Modern protocol: send name, content address method, references, repair
            name.serialize(conn, version).await?;

            // Build content address method string
            let cam_str = match method {
                FileIngestionMethod::Flat => format!("fixed:{}", hash_algo.name()),
                FileIngestionMethod::Recursive => format!("fixed:r:{}", hash_algo.name()),
            };
            cam_str.as_bytes().serialize(conn, version).await?;

            references.serialize(conn, version).await?;
            repair.serialize(conn, version).await?;
        } else {
            // Legacy protocol: send name, fixed, recursive, hash algo string
            name.serialize(conn, version).await?;

            // "fixed" flag is inverted logic: true unless SHA256 + Recursive
            let fixed = !(matches!(hash_algo, HashAlgo::Sha256)
                && matches!(method, FileIngestionMethod::Recursive));
            fixed.serialize(conn, version).await?;

            // Recursive flag
            matches!(method, FileIngestionMethod::Recursive)
                .serialize(conn, version)
                .await?;

            // Hash algorithm as string
            hash_algo.name().as_bytes().serialize(conn, version).await?;
        }

        // Now stream the data using framed format
        let mut framed_sink = FramedSink::new(conn, 8192);

        // Copy from source to framed sink
        let mut buffer = vec![0u8; 8192];
        loop {
            let n = source
                .read(&mut buffer)
                .await
                .map_err(|e| ProtocolError::Io {
                    context: "Failed to read from source".to_string(),
                    source: e,
                })?;

            if n == 0 {
                break;
            }

            framed_sink.write(&buffer[..n]).await?;
        }

        // Finish the framed stream (sends terminating zero chunk)
        let conn = framed_sink.finish().await?;

        // Process any stderr messages
        conn.process_stderr().await?;

        // Read the response (StorePath)
        StorePath::deserialize(conn, version).await
    }

    pub async fn add_to_store_nar<R: AsyncRead + Unpin>(
        &self,
        path: &StorePath,
        info: &ValidPathInfo,
        mut source: R,
        repair: bool,
        check_sigs: bool,
    ) -> Result<(), ProtocolError> {
        let mut guard = self.pool.acquire().await?;
        let (conn, version) = guard.connection_and_version();

        // Send operation code
        conn.send_opcode(OpCode::AddToStoreNar).await?;

        // Send the store path
        path.serialize(conn, version).await?;

        // Send the metadata (ValidPathInfo)
        info.serialize(conn, version).await?;

        // Send repair and check_sigs flags
        repair.serialize(conn, version).await?;
        check_sigs.serialize(conn, version).await?;

        // Now stream the NAR data using framed format
        let mut framed_sink = FramedSink::new(conn, 8192);

        // Copy from source to framed sink
        let mut buffer = vec![0u8; 8192];
        loop {
            let n = source
                .read(&mut buffer)
                .await
                .map_err(|e| ProtocolError::Io {
                    context: "Failed to read NAR data".to_string(),
                    source: e,
                })?;

            if n == 0 {
                break;
            }

            framed_sink.write(&buffer[..n]).await?;
        }

        // Finish the framed stream
        let conn = framed_sink.finish().await?;

        // Process any stderr messages
        conn.process_stderr().await?;

        // This operation returns unit
        Ok(())
    }

    pub async fn add_text_to_store(
        &self,
        name: &[u8],
        content: &[u8],
        references: &BTreeSet<StorePath>,
        repair: bool,
    ) -> Result<StorePath, ProtocolError> {
        // Note: This is obsolete since protocol 1.25, use add_to_store instead
        // But we implement it for compatibility
        let request = AddTextToStoreRequest {
            name,
            content,
            references,
            repair,
        };
        self.execute_operation(OpCode::AddTextToStore, &request)
            .await
    }

    pub async fn add_signatures(
        &self,
        path: &StorePath,
        signatures: &[NarSignature],
    ) -> Result<(), ProtocolError> {
        let request = AddSignaturesRequest { path, signatures };
        self.execute_operation(OpCode::AddSignatures, &request)
            .await
    }

    pub async fn add_temp_root(&self, path: &StorePath) -> Result<(), ProtocolError> {
        self.execute_operation(OpCode::AddTempRoot, path).await
    }

    pub async fn add_indirect_root(&self, path: &Path) -> Result<(), ProtocolError> {
        use std::os::unix::ffi::OsStrExt;
        // Get raw bytes from Path
        self.execute_operation(OpCode::AddIndirectRoot, &path.as_os_str().as_bytes())
            .await
    }

    async fn execute_operation<Req: Serialize + ?Sized, Resp: Deserialize>(
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
                request.serialize(conn, version).await?;
                conn.process_stderr().await?;
                Resp::deserialize(conn, version).await
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
