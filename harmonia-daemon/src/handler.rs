// SPDX-FileCopyrightText: 2025 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! Request handler for the local store daemon.
//!
//! This module provides the `LocalStoreHandler` which implements the daemon
//! protocol by querying the Nix store database via `harmonia-store-db`.

use std::collections::BTreeSet;
use std::future::ready;
use std::num::NonZero;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use tokio::io::AsyncBufRead;
use tokio::sync::Mutex;

use harmonia_protocol::NarHash;
use harmonia_protocol::daemon::{
    AddToStoreItem, DaemonError as ProtocolError, DaemonResult, DaemonStore, FutureResultExt,
    HandshakeDaemonStore, ResultLog, ResultLogExt as _, TrustLevel,
};
use harmonia_protocol::valid_path_info::{UnkeyedValidPathInfo, ValidPathInfo};
use harmonia_store_core::signature::{PublicKey, fingerprint_path};
use harmonia_store_core::store_path::{StoreDir, StorePath, StorePathHash};
use harmonia_store_db::StoreDb;
use harmonia_utils_hash::Hash;
use harmonia_utils_hash::fmt::{Any, CommonHash as _};

use crate::error::DaemonError;

/// A local store handler that reads from the Nix store database.
#[derive(Clone)]
pub struct LocalStoreHandler {
    store_dir: StoreDir,
    db: Arc<Mutex<StoreDb>>,
    trusted_public_keys: Arc<Vec<PublicKey>>,
}

impl LocalStoreHandler {
    /// Create a new handler with the given store directory and database path.
    pub async fn new(store_dir: StoreDir, db_path: PathBuf) -> Result<Self, DaemonError> {
        tracing::debug!("Opening database at {}", db_path.display());
        let db = StoreDb::open(&db_path, harmonia_store_db::OpenMode::ReadOnly).map_err(|e| {
            DaemonError::Database(format!("Failed to open {}: {e}", db_path.display()))
        })?;
        Ok(Self {
            store_dir,
            db: Arc::new(Mutex::new(db)),
            trusted_public_keys: Arc::new(Vec::new()),
        })
    }

    /// Create a handler from a shared database handle.
    ///
    /// Useful for tests where the caller needs to set up data in the same DB
    /// that the handler queries.
    pub fn from_shared_db(store_dir: StoreDir, db: Arc<Mutex<StoreDb>>) -> Self {
        Self {
            store_dir,
            db,
            trusted_public_keys: Arc::new(Vec::new()),
        }
    }

    /// Create a handler from a shared database handle with trusted public keys.
    pub fn from_shared_db_with_keys(
        store_dir: StoreDir,
        db: Arc<Mutex<StoreDb>>,
        trusted_public_keys: Vec<PublicKey>,
    ) -> Self {
        Self {
            store_dir,
            db,
            trusted_public_keys: Arc::new(trusted_public_keys),
        }
    }

    /// Convert a harmonia_store_db::ValidPathInfo to the protocol UnkeyedValidPathInfo.
    fn to_protocol_path_info(
        info: harmonia_store_db::ValidPathInfo,
        store_dir: StoreDir,
    ) -> Result<UnkeyedValidPathInfo, ProtocolError> {
        // Parse the hash from database format (e.g., "sha256:...")
        let hash_any = info.hash.parse::<Any<Hash>>().map_err(|e| {
            ProtocolError::custom(format!("Failed to parse hash '{}': {e}", info.hash))
        })?;
        let nar_hash = NarHash::try_from(hash_any.into_hash()).map_err(|e| {
            ProtocolError::custom(format!("Failed to convert hash '{}': {e}", info.hash))
        })?;

        // Convert references from String to StorePath
        let references = info
            .references
            .iter()
            .filter_map(|path| {
                // References are stored as full paths, extract just the name
                let base_name = std::path::Path::new(path)
                    .file_name()
                    .and_then(|n| n.to_str())?;
                StorePath::from_base_path(base_name).ok()
            })
            .collect();

        // Convert deriver
        let deriver = info.deriver.as_ref().and_then(|d| {
            let base_name = std::path::Path::new(d)
                .file_name()
                .and_then(|n| n.to_str())?;
            StorePath::from_base_path(base_name).ok()
        });

        // Convert registration time
        let registration_time = info
            .registration_time
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| NonZero::new(d.as_secs() as i64))
            .unwrap_or(None);

        // Parse signatures
        let signatures = info
            .sigs
            .map(|s| {
                s.split_whitespace()
                    .filter_map(|sig| sig.parse().ok())
                    .collect()
            })
            .unwrap_or_default();

        // Parse content address
        let ca = info.ca.and_then(|s| s.parse().ok());

        Ok(UnkeyedValidPathInfo {
            deriver,
            nar_hash,
            references,
            registration_time,
            nar_size: info.nar_size.unwrap_or(0),
            ultimate: info.ultimate,
            signatures,
            ca,
            store_dir,
        })
    }
}

impl LocalStoreHandler {
    pub(crate) fn nar_from_path_impl(
        store_dir: &StoreDir,
        path: &StorePath,
    ) -> impl ResultLog<
        Output = DaemonResult<
            tokio::io::BufReader<
                tokio_util::io::StreamReader<harmonia_nar::NarByteStream, bytes::Bytes>,
            >,
        >,
    > + Send {
        let dest_path = store_dir.to_path().join(path.to_string());

        let result = if !dest_path.exists() {
            Err(ProtocolError::custom(format!(
                "Path does not exist: {}",
                dest_path.display()
            )))
        } else {
            let byte_stream = harmonia_nar::NarByteStream::new(dest_path);
            Ok(tokio::io::BufReader::new(
                tokio_util::io::StreamReader::new(byte_stream),
            ))
        };
        ready(result).empty_logs()
    }
}

impl HandshakeDaemonStore for LocalStoreHandler {
    type Store = Self;

    fn handshake(self) -> impl ResultLog<Output = DaemonResult<Self::Store>> + Send {
        ready(Ok(self)).empty_logs()
    }
}

impl DaemonStore for LocalStoreHandler {
    fn trust_level(&self) -> TrustLevel {
        TrustLevel::Trusted
    }

    fn is_valid_path<'a>(
        &'a mut self,
        path: &'a StorePath,
    ) -> impl ResultLog<Output = DaemonResult<bool>> + Send + 'a {
        async move {
            let full_path = format!("{}/{}", self.store_dir, path);
            let db = self.db.clone();
            tokio::task::spawn_blocking(move || {
                let db = db.blocking_lock();
                db.is_valid_path(&full_path)
            })
            .await
            .map_err(|e| ProtocolError::custom(format!("Task join error: {e}")))?
            .map_err(|e| ProtocolError::custom(format!("Database error: {e}")))
        }
        .empty_logs()
    }

    fn query_path_info<'a>(
        &'a mut self,
        path: &'a StorePath,
    ) -> impl ResultLog<Output = DaemonResult<Option<UnkeyedValidPathInfo>>> + Send + 'a {
        async move {
            let full_path = format!("{}/{}", self.store_dir, path);
            let db = self.db.clone();
            let store_dir = self.store_dir.clone();
            let result = tokio::task::spawn_blocking(move || {
                let db = db.blocking_lock();
                db.query_path_info(&full_path)
            })
            .await
            .map_err(|e| ProtocolError::custom(format!("Task join error: {e}")))?
            .map_err(|e| ProtocolError::custom(format!("Database error: {e}")))?;

            result
                .map(|info| Self::to_protocol_path_info(info, store_dir))
                .transpose()
        }
        .empty_logs()
    }

    fn query_path_from_hash_part<'a>(
        &'a mut self,
        hash: &'a StorePathHash,
    ) -> impl ResultLog<Output = DaemonResult<Option<StorePath>>> + Send + 'a {
        async move {
            let hash_str = hash.to_string();
            let db = self.db.clone();
            let store_dir = self.store_dir.to_str().to_string();
            let result = tokio::task::spawn_blocking(move || {
                let db = db.blocking_lock();
                db.query_path_from_hash_part(&store_dir, &hash_str)
            })
            .await
            .map_err(|e| ProtocolError::custom(format!("Task join error: {e}")))?
            .map_err(|e| ProtocolError::custom(format!("Database error: {e}")))?;

            Ok(result.and_then(|path| {
                let base_name = std::path::Path::new(&path)
                    .file_name()
                    .and_then(|n| n.to_str())?;
                StorePath::from_base_path(base_name).ok()
            }))
        }
        .empty_logs()
    }

    fn query_valid_paths<'a>(
        &'a mut self,
        paths: &'a harmonia_store_core::store_path::StorePathSet,
        _substitute: bool,
    ) -> impl ResultLog<Output = DaemonResult<harmonia_store_core::store_path::StorePathSet>> + Send + 'a
    {
        async move {
            let mut valid = BTreeSet::new();
            for path in paths {
                let full_path = format!("{}/{}", self.store_dir, path);
                let db = self.db.clone();
                let is_valid = tokio::task::spawn_blocking(move || {
                    let db = db.blocking_lock();
                    db.is_valid_path(&full_path)
                })
                .await
                .map_err(|e| ProtocolError::custom(format!("Task join error: {e}")))?
                .map_err(|e| ProtocolError::custom(format!("Database error: {e}")))?;

                if is_valid {
                    valid.insert(path.clone());
                }
            }
            Ok(valid)
        }
        .empty_logs()
    }

    fn add_to_store_nar<'s, 'r, 'i, R>(
        &'s mut self,
        info: &'i ValidPathInfo,
        mut source: R,
        repair: bool,
        dont_check_sigs: bool,
    ) -> Pin<Box<dyn ResultLog<Output = DaemonResult<()>> + Send + 'r>>
    where
        R: AsyncBufRead + Send + Unpin + 'r,
        's: 'r,
        'i: 'r,
    {
        let store_dir = self.store_dir.clone();
        let db = self.db.clone();
        let trusted_keys = self.trusted_public_keys.clone();
        let path = info.path.clone();
        let expected_hash = info.info.nar_hash;
        let nar_size = info.info.nar_size;
        let deriver = info.info.deriver.clone();
        let references = info.info.references.clone();
        let signatures = info.info.signatures.clone();
        let ca = info.info.ca.clone();
        let ultimate = info.info.ultimate;

        async move {
            let dest_path = store_dir.to_path().join(path.to_string());
            let full_path = dest_path.to_string_lossy().to_string();

            // Check if path already exists (idempotent)
            {
                let db = db.clone();
                let full_path = full_path.clone();
                let exists = tokio::task::spawn_blocking(move || {
                    let db = db.blocking_lock();
                    db.is_valid_path(&full_path)
                })
                .await
                .map_err(|e| ProtocolError::custom(format!("Task join error: {e}")))?
                .map_err(|e| ProtocolError::custom(format!("Database error: {e}")))?;

                if exists && !repair {
                    // Drain the source to satisfy protocol expectations
                    tokio::io::copy(&mut source, &mut tokio::io::sink())
                        .await
                        .map_err(|e| ProtocolError::custom(format!("IO error: {e}")))?;
                    return Ok(());
                }
            }

            // Unpack NAR into a temp directory that auto-cleans on drop
            // (handles all error paths).  We restore into a child path so
            // that a single-file NAR can create it fresh.
            let temp_dir = tempfile::tempdir_in(dest_path.parent().unwrap_or(std::path::Path::new("/")))
                .map_err(|e| ProtocolError::custom(format!("Failed to create temp dir: {e}")))?;
            let temp_dest = temp_dir.path().join("nar");

            // Stream the NAR directly into restore while computing the
            // SHA-256 hash on the fly — matching Nix's TeeSource/HashSink
            // pattern.  This avoids buffering the entire NAR in memory or
            // on disk.
            let (hashing_reader, hash_state) =
                crate::hashing_reader::HashingReader::new(source);
            let events = harmonia_nar::parse_nar(hashing_reader);
            use futures::StreamExt as _;
            let mapped = events.map(|item| match item {
                Ok(event) => Ok(event),
                Err(e) => Err(harmonia_nar::NarWriteError::create_file_error(
                    temp_dest.clone(),
                    e,
                )),
            });
            harmonia_nar::restore(mapped, &temp_dest)
                .await
                .map_err(|e| ProtocolError::custom(format!("NAR restore error: {e}")))?;

            // Verify hash and size now that the entire NAR has been streamed.
            // On mismatch, temp_dir drop cleans up automatically.
            let hash_state = Arc::try_unwrap(hash_state)
                .expect("HashingReader should be the only other Arc holder and is consumed")
                .into_inner()
                .unwrap();
            let total_bytes = hash_state.bytes_read;
            let digest = hash_state.finish();
            let actual_hash = NarHash::from_slice(digest.as_ref())
                .map_err(|e| ProtocolError::custom(format!("Hash conversion error: {e}")))?;

            if actual_hash != expected_hash {
                return Err(ProtocolError::custom(format!(
                    "NAR hash mismatch for {}: expected {:x}, got {:x}",
                    path, expected_hash, actual_hash,
                )));
            }

            if nar_size > 0 && total_bytes != nar_size {
                return Err(ProtocolError::custom(format!(
                    "NAR size mismatch for {}: expected {}, got {}",
                    path, nar_size, total_bytes,
                )));
            }

            // Verify signatures if required
            if !dont_check_sigs {
                let nar_hash_str = format!("{}", actual_hash.as_base32());
                let fp = fingerprint_path(
                    &store_dir,
                    &path,
                    nar_hash_str.as_bytes(),
                    nar_size,
                    &references,
                )
                .map_err(|e| ProtocolError::custom(format!("Fingerprint error: {e}")))?;

                let has_valid_sig = signatures
                    .iter()
                    .any(|sig| trusted_keys.iter().any(|key| key.verify(&fp, sig)));
                if !has_valid_sig {
                    return Err(ProtocolError::custom(format!(
                        "No valid signature for {path}",
                    )));
                }
            }

            // Acquire a filesystem lock on the output path, matching Nix's
            // PathLocks pattern.  This prevents races between concurrent
            // imports of the same path (including across processes / GC).
            let full_path_for_reg = full_path.clone();
            let db = db.clone();
            tokio::task::spawn_blocking(move || {
                let _output_lock = crate::pathlocks::PathLock::lock(&dest_path)
                    .map_err(|e| ProtocolError::custom(format!("Path lock error: {e}")))?;

                let mut db = db.blocking_lock();

                // Re-check under lock — another task may have registered it
                if !repair {
                    if let Ok(true) = db.is_valid_path(&full_path_for_reg) {
                        // temp_dir drops here, cleaning up temp_dest
                        return Ok(());
                    }
                }

                // Move into place
                if dest_path.exists() && repair {
                    std::fs::remove_dir_all(&dest_path).ok();
                    std::fs::remove_file(&dest_path).ok();
                }
                std::fs::rename(&temp_dest, &dest_path).map_err(|e| {
                    // temp_dir drops on the error path, cleaning up
                    ProtocolError::custom(format!(
                        "Failed to rename to {}: {e}",
                        dest_path.display()
                    ))
                })?;
                // Disarm auto-cleanup — the content has been renamed into place
                let _ = temp_dir.keep();

                // Register in database
                let hash_str = format!("{}", expected_hash.as_base16());
                let sigs_str = if signatures.is_empty() {
                    None
                } else {
                    Some(
                        signatures
                            .iter()
                            .map(|s| s.to_string())
                            .collect::<Vec<_>>()
                            .join(" "),
                    )
                };
                let ca_str = ca.map(|c| c.to_string());
                let deriver_str = deriver.map(|d| {
                    store_dir
                        .to_path()
                        .join(d.to_string())
                        .to_string_lossy()
                        .to_string()
                });
                let refs: BTreeSet<String> = references
                    .iter()
                    .map(|r| {
                        store_dir
                            .to_path()
                            .join(r.to_string())
                            .to_string_lossy()
                            .to_string()
                    })
                    .collect();

                let params = harmonia_store_db::RegisterPathParams {
                    path: full_path_for_reg,
                    hash: hash_str,
                    registration_time: std::time::SystemTime::now(),
                    deriver: deriver_str,
                    nar_size: Some(nar_size),
                    ultimate,
                    sigs: sigs_str,
                    ca: ca_str,
                    references: refs,
                };

                db.register_valid_path(&params)
                    .map_err(|e| ProtocolError::custom(format!("Database error: {e}")))?;
                Ok(())
            })
            .await
            .map_err(|e| ProtocolError::custom(format!("Task join error: {e}")))?
        }
        .empty_logs()
        .boxed_result()
    }

    fn add_multiple_to_store<'s, 'i, 'r, S, R>(
        &'s mut self,
        repair: bool,
        dont_check_sigs: bool,
        stream: S,
    ) -> Pin<Box<dyn ResultLog<Output = DaemonResult<()>> + Send + 'r>>
    where
        S: futures::Stream<Item = Result<AddToStoreItem<R>, ProtocolError>> + Send + 'i,
        R: AsyncBufRead + Send + Unpin + 'i,
        's: 'r,
        'i: 'r,
    {
        use futures::StreamExt as _;
        let mut handler = self.clone();
        async move {
            futures::pin_mut!(stream);
            while let Some(item) = stream.next().await {
                let item = item?;
                handler
                    .add_to_store_nar(&item.info, item.reader, repair, dont_check_sigs)
                    .await?;
            }
            Ok(())
        }
        .empty_logs()
        .boxed_result()
    }

    fn shutdown(&mut self) -> impl std::future::Future<Output = DaemonResult<()>> + Send + '_ {
        ready(Ok(()))
    }
}
