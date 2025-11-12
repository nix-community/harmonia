use harmonia_store_core::hash::{Hash, fmt::Any};
use harmonia_store_remote_legacy::{
    error::ProtocolError,
    protocol::{StorePath, ValidPathInfo},
};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::BTreeSet;
use std::path::Path;

use crate::error::{DaemonError, DbContext};

/// Helper function to parse a store path from a full path string
/// e.g., "/nix/store/hash-name" -> StorePath
fn parse_store_path_from_full_path(full_path: &str) -> Result<StorePath, ProtocolError> {
    // Find the last '/' to get just the "hash-name" part
    let base_name = full_path
        .rsplit('/')
        .next()
        .ok_or_else(|| ProtocolError::DaemonError {
            message: format!("Invalid store path format: {}", full_path),
        })?;

    StorePath::from_bytes(base_name.as_bytes()).map_err(|e| ProtocolError::DaemonError {
        message: format!("Failed to parse store path '{}': {}", base_name, e),
    })
}

/// Helper trait for adding context to database errors and converting to ProtocolError
trait DbProtocolContext<T> {
    fn db_protocol_context<F>(self, f: F) -> Result<T, ProtocolError>
    where
        F: FnOnce() -> String;
}

impl<T> DbProtocolContext<T> for Result<T, rusqlite::Error> {
    fn db_protocol_context<F>(self, f: F) -> Result<T, ProtocolError>
    where
        F: FnOnce() -> String,
    {
        self.map_err(|e| ProtocolError::DaemonError {
            message: format!("{}: {}", f(), e),
        })
    }
}

pub struct StoreDb {
    conn: Connection,
}

impl StoreDb {
    pub fn open(db_path: &Path) -> Result<Self, DaemonError> {
        let conn = Connection::open(db_path)
            .db_context(|| format!("Failed to open database at {}", db_path.display()))?;
        // Set some pragmas for better performance
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;",
        )
        .db_context(|| "Failed to set database pragmas".to_string())?;
        Ok(Self { conn })
    }

    pub fn query_path_info(
        &self,
        store_path: &Path,
    ) -> Result<Option<ValidPathInfo>, ProtocolError> {
        let path_str = store_path
            .to_str()
            .ok_or_else(|| ProtocolError::DaemonError {
                message: "Invalid UTF-8 in store path".to_string(),
            })?;

        // First query the main path info
        let mut stmt = self
            .conn
            .prepare_cached(
                "SELECT id, hash, registrationTime, deriver, narSize, ultimate, sigs, ca 
             FROM ValidPaths
             WHERE path = ?1",
            )
            .db_protocol_context(|| format!("Failed to prepare query for path '{path_str}'"))?;

        let result = stmt
            .query_row(params![path_str], |row| {
                let id: i64 = row.get(0)?;
                let hash: String = row.get(1)?;
                let registration_time: i64 = row.get(2)?;
                let deriver: Option<String> = row.get(3)?;
                let nar_size: i64 = row.get(4)?;
                let ultimate: Option<i64> = row.get(5)?;
                let sigs: Option<String> = row.get(6)?;
                let ca: Option<String> = row.get(7)?;

                Ok((
                    id,
                    hash,
                    registration_time,
                    deriver,
                    nar_size,
                    ultimate,
                    sigs,
                    ca,
                ))
            })
            .optional()
            .db_protocol_context(|| format!("Failed to query path info for '{path_str}'"))?;

        let Some((id, hash, registration_time, deriver, nar_size, ultimate, sigs, ca)) = result
        else {
            return Ok(None);
        };

        // Query references
        let mut ref_stmt = self
            .conn
            .prepare_cached(
                "SELECT path FROM ValidPaths 
             JOIN Refs ON ValidPaths.id = Refs.reference 
             WHERE Refs.referrer = ?1",
            )
            .db_protocol_context(|| {
                format!("Failed to prepare references query for path '{path_str}'")
            })?;

        let references: BTreeSet<StorePath> = ref_stmt
            .query_map(params![id], |row| {
                let path: String = row.get(0)?;
                Ok(path)
            })
            .db_protocol_context(|| format!("Failed to query references for path '{path_str}'"))?
            .collect::<Result<Vec<_>, _>>()
            .db_protocol_context(|| format!("Failed to collect references for path '{path_str}'"))?
            .into_iter()
            .map(|path| parse_store_path_from_full_path(&path))
            .collect::<Result<BTreeSet<_>, _>>()?;

        // Parse the hash from database format
        let parsed_hash = hash
            .parse::<Any<Hash>>()
            .map_err(|e| ProtocolError::DaemonError {
                message: format!("Failed to parse hash from database '{hash}': {e}"),
            })?
            .into_hash();

        // Build ValidPathInfo
        let info = ValidPathInfo {
            deriver: deriver
                .map(|s| parse_store_path_from_full_path(&s))
                .transpose()?,
            hash: parsed_hash,
            references,
            registration_time: registration_time as u64,
            nar_size: nar_size as u64,
            ultimate: ultimate.unwrap_or(0) != 0,
            signatures: sigs
                .map(|s| {
                    s.split_whitespace()
                        .map(|sig| sig.as_bytes().to_vec())
                        .collect()
                })
                .unwrap_or_default(),
            content_address: ca.map(|s| s.into_bytes()),
        };

        Ok(Some(info))
    }

    pub fn query_path_from_hash_part(
        &self,
        store_dir: &Path,
        hash_part: &str,
    ) -> Result<Option<StorePath>, ProtocolError> {
        // Construct the prefix to search for
        let prefix = format!("{}/{}", store_dir.display(), hash_part);

        let mut stmt = self
            .conn
            .prepare_cached("SELECT path FROM ValidPaths WHERE path >= ?1 ORDER BY path LIMIT 1")
            .db_protocol_context(|| {
                format!("Failed to prepare query for hash part '{hash_part}'")
            })?;

        let result = stmt
            .query_row(params![&prefix], |row| {
                let path: String = row.get(0)?;
                Ok(path)
            })
            .optional()
            .db_protocol_context(|| {
                format!(
                    "Failed to execute query for hash part '{hash_part}' with prefix '{prefix}'"
                )
            })?;

        if let Some(path) = result {
            // Check if it actually starts with our prefix
            if path.starts_with(&prefix) {
                Ok(Some(parse_store_path_from_full_path(&path)?))
            } else {
                Ok(None)
            }
        } else {
            Ok(None)
        }
    }

    pub fn is_valid_path(&self, store_path: &Path) -> Result<bool, ProtocolError> {
        let path_str = store_path
            .to_str()
            .ok_or_else(|| ProtocolError::DaemonError {
                message: "Invalid UTF-8 in store path".to_string(),
            })?;

        let mut stmt = self
            .conn
            .prepare_cached("SELECT 1 FROM ValidPaths WHERE path = ?1 LIMIT 1")
            .db_protocol_context(|| {
                format!("Failed to prepare validity check for path '{path_str}'")
            })?;

        let exists = stmt
            .exists(params![path_str])
            .db_protocol_context(|| format!("Failed to check validity of path '{path_str}'"))?;

        Ok(exists)
    }
}
