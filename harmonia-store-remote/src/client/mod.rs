pub mod connection;
pub mod metrics;
pub mod pool;

use crate::error::{IoErrorContext, ProtocolError};
use crate::framed::FramedSink;
use crate::protocol::{
    OpCode, ValidPathInfo,
    types::{
        AddSignaturesRequest, AddTextToStoreRequest, BasicDerivation, BuildMode, BuildResult,
        DaemonSettings, DerivedPath, DrvOutputId, GCOptions, GCResult, GCRoot, Missing,
        Realisation, SubstitutablePathInfo, SubstitutablePathInfos, VerifyStoreRequest,
    },
};
use crate::serialization::{Deserialize, Serialize};
use harmonia_store_core::{FileIngestionMethod, HashAlgo, NarSignature, StorePath};
use pool::ConnectionPool;
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};

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

    // Build Operations

    pub async fn build_paths(
        &self,
        paths: &[DerivedPath],
        mode: BuildMode,
    ) -> Result<(), ProtocolError> {
        let mut guard = self.pool.acquire().await?;
        let (conn, version) = guard.connection_and_version();

        // Send operation code
        conn.send_opcode(OpCode::BuildPaths).await?;

        // Send the paths
        paths.serialize(conn, version).await?;

        // BuildMode is only sent for protocol >= 1.15
        if version.minor >= 15 {
            mode.serialize(conn, version).await?;
        }

        // Process any stderr messages
        conn.process_stderr().await?;

        // This operation returns unit
        Ok(())
    }

    pub async fn build_derivation(
        &self,
        drv_path: &StorePath,
        drv: &BasicDerivation,
        mode: BuildMode,
    ) -> Result<BuildResult, ProtocolError> {
        let mut guard = self.pool.acquire().await?;
        let (conn, version) = guard.connection_and_version();

        // Send operation code
        conn.send_opcode(OpCode::BuildDerivation).await?;

        // Send the derivation path
        drv_path.serialize(conn, version).await?;

        // Send the basic derivation
        drv.serialize(conn, version).await?;

        // Send the build mode
        mode.serialize(conn, version).await?;

        // Process any stderr messages
        conn.process_stderr().await?;

        // Read the build result
        BuildResult::deserialize(conn, version).await
    }

    pub async fn ensure_path(&self, path: &StorePath) -> Result<(), ProtocolError> {
        self.execute_operation(OpCode::EnsurePath, path).await
    }

    /// Perform garbage collection
    pub async fn collect_garbage(&self, options: &GCOptions) -> Result<GCResult, ProtocolError> {
        self.execute_operation(OpCode::CollectGarbage, options)
            .await
    }

    /// Find garbage collector roots
    pub async fn find_roots(
        &self,
    ) -> Result<std::collections::BTreeMap<StorePath, GCRoot>, ProtocolError> {
        self.execute_operation(OpCode::FindRoots, &()).await
    }

    /// Optimise the Nix store by hard-linking identical files
    pub async fn optimise_store(&self) -> Result<(), ProtocolError> {
        self.execute_operation(OpCode::OptimiseStore, &()).await
    }

    /// Synchronize with the garbage collector
    pub async fn sync_with_gc(&self) -> Result<(), ProtocolError> {
        self.execute_operation(OpCode::SyncWithGC, &()).await
    }

    /// Verify store integrity
    pub async fn verify_store(
        &self,
        check_contents: bool,
        repair: bool,
    ) -> Result<bool, ProtocolError> {
        let request = VerifyStoreRequest {
            check_contents,
            repair,
        };
        self.execute_operation(OpCode::VerifyStore, &request).await
    }

    /// Export NAR from a store path
    pub async fn nar_from_path<W: AsyncWrite + Unpin>(
        &self,
        path: &StorePath,
        sink: &mut W,
    ) -> Result<(), ProtocolError> {
        let mut guard = self.pool.acquire().await?;
        let (conn, version) = guard.connection_and_version();

        // Send operation
        conn.send_opcode(OpCode::NarFromPath).await?;
        path.serialize(conn, version).await?;
        conn.process_stderr().await?;

        // Stream NAR data to sink
        tokio::io::copy(conn, sink)
            .await
            .io_context("Failed to stream NAR data")?;

        Ok(())
    }

    /// Check if substitutes are available for a given path
    pub async fn has_substitutes(&self, path: &StorePath) -> Result<bool, ProtocolError> {
        self.execute_operation(OpCode::HasSubstitutes, path).await
    }

    /// Query failed paths
    pub async fn query_failed_paths(&self) -> Result<Vec<StorePath>, ProtocolError> {
        self.execute_operation(OpCode::QueryFailedPaths, &()).await
    }

    /// Clear failed paths from the failed paths cache
    pub async fn clear_failed_paths(&self, paths: &[StorePath]) -> Result<(), ProtocolError> {
        self.execute_operation(OpCode::ClearFailedPaths, &paths)
            .await
    }

    /// Query derivation output map
    pub async fn query_derivation_output_map(
        &self,
        drv: &StorePath,
    ) -> Result<std::collections::BTreeMap<Vec<u8>, Option<StorePath>>, ProtocolError> {
        self.execute_operation(OpCode::QueryDerivationOutputMap, drv)
            .await
    }

    /// Build paths with results
    pub async fn build_paths_with_results(
        &self,
        paths: &[DerivedPath],
        mode: BuildMode,
    ) -> Result<Vec<BuildResult>, ProtocolError> {
        let mut guard = self.pool.acquire().await?;
        let (conn, version) = guard.connection_and_version();

        // Send operation code
        conn.send_opcode(OpCode::BuildPathsWithResults).await?;

        // Send the paths
        paths.serialize(conn, version).await?;

        // BuildMode is only sent for protocol >= 1.15
        if version.minor >= 15 {
            mode.serialize(conn, version).await?;
        }

        // Process any stderr messages
        conn.process_stderr().await?;

        // Read the results
        Vec::<BuildResult>::deserialize(conn, version).await
    }

    /// Add a permanent garbage collector root
    pub async fn add_perm_root(
        &self,
        path: &StorePath,
        gc_root: &Path,
    ) -> Result<(), ProtocolError> {
        use std::os::unix::ffi::OsStrExt;

        let mut guard = self.pool.acquire().await?;
        let (conn, version) = guard.connection_and_version();

        // Send operation code
        conn.send_opcode(OpCode::AddPermRoot).await?;

        // Send the store path
        path.serialize(conn, version).await?;

        // Send the GC root path as bytes
        gc_root
            .as_os_str()
            .as_bytes()
            .serialize(conn, version)
            .await?;

        // Process any stderr messages
        conn.process_stderr().await?;

        Ok(())
    }

    /// Set daemon options
    pub async fn set_options(&self, settings: &DaemonSettings) -> Result<(), ProtocolError> {
        let mut guard = self.pool.acquire().await?;
        let (conn, version) = guard.connection_and_version();

        // Send operation code
        conn.send_opcode(OpCode::SetOptions).await?;

        // Send settings
        settings.keep_going.serialize(conn, version).await?;
        settings.keep_failed.serialize(conn, version).await?;
        settings.try_fallback.serialize(conn, version).await?;
        settings.verbosity.serialize(conn, version).await?;
        settings.max_build_jobs.serialize(conn, version).await?;
        settings.max_silent_time.serialize(conn, version).await?;
        settings.use_build_hook.serialize(conn, version).await?;

        // Handle version-specific fields
        if version.minor >= 2 {
            settings.build_cores.serialize(conn, version).await?;
        }
        if version.minor >= 3 {
            settings.use_substitutes.serialize(conn, version).await?;
        }
        if version.minor >= 4 {
            // Empty build users groups for compatibility
            let empty_users: Vec<Vec<u8>> = vec![];
            empty_users.as_slice().serialize(conn, version).await?;
        }
        if version.minor >= 6 && version.minor < 11 {
            // Legacy auto-args - just send 0
            0u64.serialize(conn, version).await?;
        }
        if version.minor >= 10 {
            // Send build hook
            if let Some(ref hook) = settings.build_hook {
                1u64.serialize(conn, version).await?;
                hook.serialize(conn, version).await?;
            } else {
                0u64.serialize(conn, version).await?;
            }
        }
        if version.minor >= 12 {
            // Send substitute URLs
            settings
                .substitute_urls
                .as_slice()
                .serialize(conn, version)
                .await?;
        }

        // Process any stderr messages
        conn.process_stderr().await?;

        Ok(())
    }

    /// Query substitutable path info for a single path
    pub async fn query_substitutable_path_info(
        &self,
        path: &StorePath,
    ) -> Result<Option<SubstitutablePathInfo>, ProtocolError> {
        self.execute_operation(OpCode::QuerySubstitutablePathInfo, path)
            .await
    }

    /// Query substitutable path info for multiple paths
    pub async fn query_substitutable_path_infos(
        &self,
        paths: &[StorePath],
    ) -> Result<SubstitutablePathInfos, ProtocolError> {
        self.execute_operation(OpCode::QuerySubstitutablePathInfos, &paths)
            .await
    }

    /// Register a derivation output
    pub async fn register_drv_output(
        &self,
        realisation: &Realisation,
    ) -> Result<(), ProtocolError> {
        self.execute_operation(OpCode::RegisterDrvOutput, realisation)
            .await
    }

    /// Query a realisation
    pub async fn query_realisation(
        &self,
        id: &DrvOutputId,
    ) -> Result<Option<Realisation>, ProtocolError> {
        self.execute_operation(OpCode::QueryRealisation, id).await
    }

    /// Add multiple paths to store
    pub async fn add_multiple_to_store<R: AsyncRead + Unpin>(
        &self,
        mut source: R,
        repair: bool,
        check_sigs: bool,
    ) -> Result<(), ProtocolError> {
        let mut guard = self.pool.acquire().await?;
        let (conn, version) = guard.connection_and_version();

        // Send operation code
        conn.send_opcode(OpCode::AddMultipleToStore).await?;

        // Send repair and check_sigs flags
        repair.serialize(conn, version).await?;
        check_sigs.serialize(conn, version).await?;

        // Now stream the archive data using framed format
        let mut framed_sink = FramedSink::new(conn, 8192);

        // Copy from source to framed sink
        let mut buffer = vec![0u8; 8192];
        loop {
            let n = source
                .read(&mut buffer)
                .await
                .map_err(|e| ProtocolError::Io {
                    context: "Failed to read archive data".to_string(),
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

        Ok(())
    }

    /// Add a build log
    pub async fn add_build_log<R: AsyncRead + Unpin>(
        &self,
        drv_path: &StorePath,
        mut log: R,
    ) -> Result<(), ProtocolError> {
        let mut guard = self.pool.acquire().await?;
        let (conn, version) = guard.connection_and_version();

        // Send operation code
        conn.send_opcode(OpCode::AddBuildLog).await?;

        // Send the derivation path
        drv_path.serialize(conn, version).await?;

        // Stream log data
        let mut framed_sink = FramedSink::new(conn, 8192);
        let mut buffer = vec![0u8; 8192];

        loop {
            let n = log.read(&mut buffer).await.map_err(|e| ProtocolError::Io {
                context: "Failed to read log data".to_string(),
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

        Ok(())
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
