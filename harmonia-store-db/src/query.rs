// SPDX-FileCopyrightText: 2025 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! Read query operations for the store database.

use std::collections::BTreeSet;

use rusqlite::params;

use crate::connection::StoreDb;
use crate::error::Result;
use crate::types::{DerivationOutput, Realisation, ValidPathInfo, unix_to_system_time};

impl StoreDb {
    /// Query path info by full store path.
    ///
    /// Returns `None` if the path is not in the database.
    pub fn query_path_info(&self, path: &str) -> Result<Option<ValidPathInfo>> {
        let mut stmt = self.conn.prepare_cached(
            r#"
            SELECT id, path, hash, registrationTime, deriver, narSize, ultimate, sigs, ca
            FROM ValidPaths
            WHERE path = ?1
            "#,
        )?;

        let info = stmt.query_row(params![path], |row| {
            Ok(ValidPathInfo {
                id: row.get(0)?,
                path: row.get(1)?,
                hash: row.get(2)?,
                registration_time: unix_to_system_time(row.get(3)?),
                deriver: row.get(4)?,
                nar_size: row.get::<_, Option<i64>>(5)?.map(|n| n as u64),
                ultimate: row.get::<_, Option<i32>>(6)?.unwrap_or(0) != 0,
                sigs: row.get(7)?,
                ca: row.get(8)?,
                references: BTreeSet::new(),
            })
        });

        match info {
            Ok(mut info) => {
                // Fetch references
                info.references = self.query_references_by_id(info.id)?;
                Ok(Some(info))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Query path info by database ID.
    pub fn query_path_info_by_id(&self, id: i64) -> Result<Option<ValidPathInfo>> {
        let mut stmt = self.conn.prepare_cached(
            r#"
            SELECT id, path, hash, registrationTime, deriver, narSize, ultimate, sigs, ca
            FROM ValidPaths
            WHERE id = ?1
            "#,
        )?;

        let info = stmt.query_row(params![id], |row| {
            Ok(ValidPathInfo {
                id: row.get(0)?,
                path: row.get(1)?,
                hash: row.get(2)?,
                registration_time: unix_to_system_time(row.get(3)?),
                deriver: row.get(4)?,
                nar_size: row.get::<_, Option<i64>>(5)?.map(|n| n as u64),
                ultimate: row.get::<_, Option<i32>>(6)?.unwrap_or(0) != 0,
                sigs: row.get(7)?,
                ca: row.get(8)?,
                references: BTreeSet::new(),
            })
        });

        match info {
            Ok(mut info) => {
                info.references = self.query_references_by_id(info.id)?;
                Ok(Some(info))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Look up a store path by its hash part (the 32-character prefix).
    ///
    /// The `store_dir` should be the store directory (e.g., "/nix/store").
    pub fn query_path_from_hash_part(
        &self,
        store_dir: &str,
        hash_part: &str,
    ) -> Result<Option<String>> {
        // Construct prefix to search for
        let prefix = format!("{store_dir}/{hash_part}");

        let mut stmt = self.conn.prepare_cached(
            r#"
            SELECT path FROM ValidPaths WHERE path >= ?1 LIMIT 1
            "#,
        )?;

        let result: Option<String> = stmt.query_row(params![&prefix], |row| row.get(0)).ok();

        // Verify the result actually starts with our prefix
        match result {
            Some(path) if path.starts_with(&prefix) => Ok(Some(path)),
            _ => Ok(None),
        }
    }

    /// Check if a store path is valid (exists in the database).
    pub fn is_valid_path(&self, path: &str) -> Result<bool> {
        let mut stmt = self.conn.prepare_cached(
            r#"
            SELECT 1 FROM ValidPaths WHERE path = ?1 LIMIT 1
            "#,
        )?;

        let exists = stmt.query_row(params![path], |_| Ok(())).is_ok();
        Ok(exists)
    }

    /// Get all paths referenced by a given path (by path string).
    pub fn query_references(&self, path: &str) -> Result<BTreeSet<String>> {
        let mut stmt = self.conn.prepare_cached(
            r#"
            SELECT v.path
            FROM Refs r
            JOIN ValidPaths v ON r.reference = v.id
            WHERE r.referrer = (SELECT id FROM ValidPaths WHERE path = ?1)
            "#,
        )?;

        let mut refs = BTreeSet::new();
        let mut rows = stmt.query(params![path])?;
        while let Some(row) = rows.next()? {
            refs.insert(row.get(0)?);
        }
        Ok(refs)
    }

    /// Get all paths referenced by a given path (by ID).
    fn query_references_by_id(&self, id: i64) -> Result<BTreeSet<String>> {
        let mut stmt = self.conn.prepare_cached(
            r#"
            SELECT v.path
            FROM Refs r
            JOIN ValidPaths v ON r.reference = v.id
            WHERE r.referrer = ?1
            "#,
        )?;

        let mut refs = BTreeSet::new();
        let mut rows = stmt.query(params![id])?;
        while let Some(row) = rows.next()? {
            refs.insert(row.get(0)?);
        }
        Ok(refs)
    }

    /// Get all paths that reference a given path (reverse dependencies).
    pub fn query_referrers(&self, path: &str) -> Result<BTreeSet<String>> {
        let mut stmt = self.conn.prepare_cached(
            r#"
            SELECT v.path
            FROM Refs r
            JOIN ValidPaths v ON r.referrer = v.id
            WHERE r.reference = (SELECT id FROM ValidPaths WHERE path = ?1)
            "#,
        )?;

        let mut refs = BTreeSet::new();
        let mut rows = stmt.query(params![path])?;
        while let Some(row) = rows.next()? {
            refs.insert(row.get(0)?);
        }
        Ok(refs)
    }

    /// Get all derivations that produced a given output path.
    pub fn query_valid_derivers(&self, output_path: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare_cached(
            r#"
            SELECT v.path
            FROM DerivationOutputs d
            JOIN ValidPaths v ON d.drv = v.id
            WHERE d.path = ?1
            "#,
        )?;

        let mut derivers = Vec::new();
        let mut rows = stmt.query(params![output_path])?;
        while let Some(row) = rows.next()? {
            derivers.push(row.get(0)?);
        }
        Ok(derivers)
    }

    /// Get all outputs of a derivation.
    pub fn query_derivation_outputs(&self, drv_path: &str) -> Result<Vec<DerivationOutput>> {
        let mut stmt = self.conn.prepare_cached(
            r#"
            SELECT d.drv, d.id, d.path
            FROM DerivationOutputs d
            JOIN ValidPaths v ON d.drv = v.id
            WHERE v.path = ?1
            "#,
        )?;

        let mut outputs = Vec::new();
        let mut rows = stmt.query(params![drv_path])?;
        while let Some(row) = rows.next()? {
            outputs.push(DerivationOutput {
                drv_id: row.get(0)?,
                output_id: row.get(1)?,
                path: row.get(2)?,
            });
        }
        Ok(outputs)
    }

    /// Get all valid paths in the database.
    ///
    /// Warning: This can be slow for large stores!
    pub fn query_all_valid_paths(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare_cached("SELECT path FROM ValidPaths")?;

        let mut paths = Vec::new();
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            paths.push(row.get(0)?);
        }
        Ok(paths)
    }

    /// Count the number of valid paths.
    pub fn count_valid_paths(&self) -> Result<u64> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM ValidPaths", [], |row| row.get(0))?;
        Ok(count as u64)
    }

    /// Query a realisation by derivation path and output name.
    pub fn query_realisation(
        &self,
        drv_path: &str,
        output_name: &str,
    ) -> Result<Option<Realisation>> {
        let mut stmt = self.conn.prepare_cached(
            r#"
            SELECT id, drvPath, outputName, outputPath, signatures
            FROM Realisations
            WHERE drvPath = ?1 AND outputName = ?2
            "#,
        )?;

        let result = stmt.query_row(params![drv_path, output_name], |row| {
            Ok(Realisation {
                id: row.get(0)?,
                drv_path: row.get(1)?,
                output_name: row.get(2)?,
                output_path_id: row.get(3)?,
                signatures: row.get(4)?,
            })
        });

        match result {
            Ok(r) => Ok(Some(r)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}
