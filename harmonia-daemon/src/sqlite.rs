use harmonia_store_core::{ContentAddress, Hash, NarSignature};
use harmonia_store_remote::{
    error::ProtocolError,
    protocol::types::{DrvOutputId, Realisation},
    protocol::{StorePath, ValidPathInfo},
};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::{BTreeMap, BTreeSet};
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

    /// Get the ID of a valid path from the database
    fn get_valid_path_id(&self, path_str: &str) -> Result<Option<i64>, ProtocolError> {
        self.conn
            .query_row(
                "SELECT id FROM ValidPaths WHERE path = ?1",
                params![path_str],
                |row| row.get(0),
            )
            .optional()
            .db_protocol_context(|| format!("Failed to get path ID for '{path_str}'"))
    }

    /// Query paths and collect them into a BTreeSet
    fn query_paths<P, F>(
        &self,
        sql: &str,
        params: P,
        error_context: F,
    ) -> Result<BTreeSet<StorePath>, ProtocolError>
    where
        P: rusqlite::Params,
        F: Fn() -> String + Clone,
    {
        let mut stmt = self
            .conn
            .prepare_cached(sql)
            .db_protocol_context(|| format!("Failed to prepare {}", error_context()))?;

        stmt.query_map(params, |row| {
            let path: String = row.get(0)?;
            Ok(StorePath::from(path.into_bytes()))
        })
        .db_protocol_context(|| format!("Failed to query {}", error_context()))?
        .collect::<Result<BTreeSet<_>, _>>()
        .db_protocol_context(|| format!("Failed to collect {}", error_context()))
    }

    pub fn query_path_info(&self, path_str: &str) -> Result<Option<ValidPathInfo>, ProtocolError> {
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
                Ok(StorePath::from(path.into_bytes()))
            })
            .db_protocol_context(|| format!("Failed to query references for path '{path_str}'"))?
            .collect::<Result<BTreeSet<_>, _>>()
            .db_protocol_context(|| {
                format!("Failed to collect references for path '{path_str}'")
            })?;

        // Parse the hash from database format
        let parsed_hash = Hash::parse(hash.as_bytes()).map_err(|e| ProtocolError::DaemonError {
            message: format!("Failed to parse hash from database '{hash}': {e}"),
        })?;

        // Build ValidPathInfo
        let info = ValidPathInfo {
            deriver: deriver.map(|s| StorePath::from(s.into_bytes())),
            hash: parsed_hash,
            references,
            registration_time: registration_time as u64,
            nar_size: nar_size as u64,
            ultimate: ultimate.unwrap_or(0) != 0,
            signatures: sigs
                .map(|s| {
                    s.split_whitespace()
                        .filter_map(|sig| NarSignature::parse(sig.as_bytes()).ok())
                        .collect()
                })
                .unwrap_or_default(),
            content_address: ca.and_then(|s| ContentAddress::parse(s.as_bytes()).ok()),
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
                Ok(Some(StorePath::from(path.into_bytes())))
            } else {
                Ok(None)
            }
        } else {
            Ok(None)
        }
    }

    pub fn is_valid_path(&self, path_str: &str) -> Result<bool, ProtocolError> {
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

    pub fn query_all_valid_paths(&self) -> Result<BTreeSet<StorePath>, ProtocolError> {
        self.query_paths("SELECT path FROM ValidPaths", [], || {
            "all valid paths".to_string()
        })
    }

    pub fn query_referrers(&self, path_str: &str) -> Result<BTreeSet<StorePath>, ProtocolError> {
        self.query_paths(
            "SELECT path FROM Refs 
             JOIN ValidPaths ON referrer = id 
             WHERE reference = (SELECT id FROM ValidPaths WHERE path = ?1)",
            params![path_str],
            || format!("referrers for '{path_str}'"),
        )
    }

    pub fn query_valid_derivers(
        &self,
        path_str: &str,
    ) -> Result<BTreeSet<StorePath>, ProtocolError> {
        self.query_paths(
            "SELECT v.path FROM DerivationOutputs d 
             JOIN ValidPaths v ON d.drv = v.id 
             WHERE d.path = ?1",
            params![path_str],
            || format!("valid derivers for '{path_str}'"),
        )
    }

    pub fn query_derivation_outputs(
        &self,
        path_str: &str,
    ) -> Result<BTreeSet<StorePath>, ProtocolError> {
        let drv_id =
            self.get_valid_path_id(path_str)?
                .ok_or_else(|| ProtocolError::DaemonError {
                    message: format!("Derivation not found: {path_str}"),
                })?;

        self.query_paths(
            "SELECT path FROM DerivationOutputs WHERE drv = ?1",
            params![drv_id],
            || format!("derivation outputs for '{path_str}'"),
        )
    }

    pub fn query_derivation_output_names(
        &self,
        path_str: &str,
    ) -> Result<Vec<Vec<u8>>, ProtocolError> {
        let drv_id =
            self.get_valid_path_id(path_str)?
                .ok_or_else(|| ProtocolError::DaemonError {
                    message: format!("Derivation not found: {path_str}"),
                })?;

        let mut stmt = self
            .conn
            .prepare_cached("SELECT id FROM DerivationOutputs WHERE drv = ?1")
            .db_protocol_context(|| {
                format!("Failed to prepare output names query for '{path_str}'")
            })?;

        let names = stmt
            .query_map(params![drv_id], |row| {
                let name: String = row.get(0)?;
                Ok(name.into_bytes())
            })
            .db_protocol_context(|| format!("Failed to query output names for '{path_str}'"))?
            .collect::<Result<Vec<_>, _>>()
            .db_protocol_context(|| format!("Failed to collect output names for '{path_str}'"))?;

        Ok(names)
    }

    pub fn query_derivation_output_map(
        &self,
        path_str: &str,
    ) -> Result<BTreeMap<String, Option<StorePath>>, ProtocolError> {
        // First get the derivation ID
        let drv_id: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM ValidPaths WHERE path = ?1",
                params![path_str],
                |row| row.get(0),
            )
            .optional()
            .db_protocol_context(|| format!("Failed to get derivation ID for '{path_str}'"))?;

        let drv_id = drv_id.ok_or_else(|| ProtocolError::DaemonError {
            message: format!("Derivation not found: {path_str}"),
        })?;

        let mut stmt = self
            .conn
            .prepare_cached("SELECT id, path FROM DerivationOutputs WHERE drv = ?1")
            .db_protocol_context(|| {
                format!("Failed to prepare output map query for '{path_str}'")
            })?;

        let outputs = stmt
            .query_map(params![drv_id], |row| {
                let name: String = row.get(0)?;
                let path: String = row.get(1)?;
                Ok((name, Some(StorePath::from(path.into_bytes()))))
            })
            .db_protocol_context(|| format!("Failed to query output map for '{path_str}'"))?
            .collect::<Result<BTreeMap<_, _>, _>>()
            .db_protocol_context(|| format!("Failed to collect output map for '{path_str}'"))?;

        Ok(outputs)
    }

    pub fn query_realisation(
        &self,
        drv_hash: &[u8],
        output_name: &str,
    ) -> Result<Option<Realisation>, ProtocolError> {
        // Convert drv_hash to string for the query (Nix stores it as text)
        let drv_path = std::str::from_utf8(drv_hash).map_err(|_| ProtocolError::DaemonError {
            message: "Invalid UTF-8 in derivation hash".to_string(),
        })?;

        let mut stmt = self
            .conn
            .prepare_cached(
                "SELECT Realisations.id, Output.path, Realisations.signatures FROM Realisations
                 INNER JOIN ValidPaths AS Output ON Output.id = Realisations.outputPath
                 WHERE drvPath = ?1 AND outputName = ?2",
            )
            .db_protocol_context(|| {
                format!("Failed to prepare realisation query for '{drv_path}:{output_name}'")
            })?;

        let result = stmt
            .query_row(params![&drv_path, output_name], |row| {
                let _id: i64 = row.get(0)?;
                let out_path: String = row.get(1)?;
                let signatures: Option<String> = row.get(2)?;

                Ok((out_path, signatures))
            })
            .optional()
            .db_protocol_context(|| {
                format!("Failed to query realisation for '{drv_path}:{output_name}'")
            })?;

        if let Some((out_path, signatures)) = result {
            // Parse signatures if present
            let sigs: BTreeSet<NarSignature> = if let Some(sig_str) = signatures {
                sig_str
                    .split(' ')
                    .filter(|s| !s.is_empty())
                    .filter_map(|s| NarSignature::parse(s.as_bytes()).ok())
                    .collect()
            } else {
                BTreeSet::new()
            };

            // Query dependent realisations from RealisationsRefs
            let realisation_id: i64 = self
                .conn
                .query_row(
                    "SELECT id FROM Realisations WHERE drvPath = ?1 AND outputName = ?2",
                    params![&drv_path, output_name],
                    |row| row.get(0),
                )
                .db_protocol_context(|| {
                    format!("Failed to get realisation ID for '{drv_path}:{output_name}'")
                })?;

            let mut deps_stmt = self
                .conn
                .prepare_cached(
                    "SELECT r.drvPath, r.outputName, v.path 
                     FROM RealisationsRefs rr
                     JOIN Realisations r ON r.id = rr.realisationReference
                     JOIN ValidPaths v ON v.id = r.outputPath
                     WHERE rr.referrer = ?1",
                )
                .db_protocol_context(|| {
                    format!("Failed to prepare dependent realisations query for '{drv_path}:{output_name}'")
                })?;

            let dependent_realisations = deps_stmt
                .query_map(params![realisation_id], |row| {
                    let dep_drv_path: String = row.get(0)?;
                    let dep_output_name: String = row.get(1)?;
                    let dep_out_path: String = row.get(2)?;

                    Ok((
                        DrvOutputId {
                            drv_hash: dep_drv_path.into_bytes(),
                            output_name: dep_output_name.into_bytes(),
                        },
                        StorePath::from(dep_out_path.into_bytes()),
                    ))
                })
                .db_protocol_context(|| {
                    format!("Failed to query dependent realisations for '{drv_path}:{output_name}'")
                })?
                .collect::<Result<BTreeMap<_, _>, _>>()
                .db_protocol_context(|| {
                    format!(
                        "Failed to collect dependent realisations for '{drv_path}:{output_name}'"
                    )
                })?;

            Ok(Some(Realisation {
                id: DrvOutputId {
                    drv_hash: drv_hash.to_vec(),
                    output_name: output_name.as_bytes().to_vec(),
                },
                out_path: StorePath::from(out_path.into_bytes()),
                signatures: sigs,
                dependent_realisations,
            }))
        } else {
            Ok(None)
        }
    }
}
