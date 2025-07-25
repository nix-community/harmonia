use harmonia_store_remote::{
    error::ProtocolError,
    protocol::{StorePath, ValidPathInfo},
};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;

use crate::error::{DaemonError, DbContext};

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

        let references: Vec<StorePath> = ref_stmt
            .query_map(params![id], |row| {
                let path: String = row.get(0)?;
                Ok(StorePath::from(path))
            })
            .db_protocol_context(|| format!("Failed to query references for path '{}'", path_str))?
            .collect::<Result<Vec<_>, _>>()
            .db_protocol_context(|| {
                format!("Failed to collect references for path '{path_str}'")
            })?;

        // Build ValidPathInfo
        let info = ValidPathInfo {
            deriver: deriver.map(StorePath::from),
            // Extract hex part of hash for protocol (Nix daemon protocol expects plain hex without prefix)
            // Nix always stores hashes with "sha256:" prefix in the database
            hash: hash.as_bytes()[7..].to_vec(),
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
                Ok(Some(StorePath::from(path)))
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
