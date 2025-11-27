// SPDX-FileCopyrightText: 2025 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! Write operations for the store database.
//!
//! These are primarily used for testing and local store management.

use std::collections::BTreeSet;
use std::time::SystemTime;

use rusqlite::params;

use crate::connection::StoreDb;
use crate::error::Result;
use crate::types::system_time_to_unix;

/// Parameters for registering a new valid path.
#[derive(Debug, Clone)]
pub struct RegisterPathParams {
    /// Full store path
    pub path: String,
    /// Base16-encoded content hash
    pub hash: String,
    /// When this path was registered
    pub registration_time: SystemTime,
    /// Derivation that produced this (if any)
    pub deriver: Option<String>,
    /// NAR size in bytes
    pub nar_size: Option<u64>,
    /// Whether built locally (not substituted)
    pub ultimate: bool,
    /// Space-separated signatures
    pub sigs: Option<String>,
    /// Content address (if content-addressed)
    pub ca: Option<String>,
    /// Paths this references
    pub references: BTreeSet<String>,
}

impl Default for RegisterPathParams {
    fn default() -> Self {
        Self {
            path: String::new(),
            hash: String::new(),
            registration_time: SystemTime::now(),
            deriver: None,
            nar_size: None,
            ultimate: false,
            sigs: None,
            ca: None,
            references: BTreeSet::new(),
        }
    }
}

impl StoreDb {
    /// Register a new valid path.
    ///
    /// Returns the database ID of the new path.
    pub fn register_valid_path(&mut self, params: &RegisterPathParams) -> Result<i64> {
        let tx = self.conn.transaction()?;

        // Insert the path
        tx.execute(
            r#"
            INSERT INTO ValidPaths (path, hash, registrationTime, deriver, narSize, ultimate, sigs, ca)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            "#,
            params![
                params.path,
                params.hash,
                system_time_to_unix(params.registration_time),
                params.deriver,
                params.nar_size.map(|n| n as i64),
                if params.ultimate { 1 } else { 0 },
                params.sigs,
                params.ca,
            ],
        )?;

        let id = tx.last_insert_rowid();

        // Add references
        for reference in &params.references {
            // Get or skip if reference doesn't exist
            let ref_id: Option<i64> = tx
                .query_row(
                    "SELECT id FROM ValidPaths WHERE path = ?1",
                    params![reference],
                    |row| row.get(0),
                )
                .ok();

            if let Some(ref_id) = ref_id {
                tx.execute(
                    "INSERT OR REPLACE INTO Refs (referrer, reference) VALUES (?1, ?2)",
                    params![id, ref_id],
                )?;
            }
        }

        tx.commit()?;
        Ok(id)
    }

    /// Add a reference from one path to another.
    ///
    /// Both paths must already exist in the database.
    pub fn add_reference(&self, referrer_path: &str, reference_path: &str) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT OR REPLACE INTO Refs (referrer, reference)
            SELECT r.id, f.id
            FROM ValidPaths r, ValidPaths f
            WHERE r.path = ?1 AND f.path = ?2
            "#,
            params![referrer_path, reference_path],
        )?;
        Ok(())
    }

    /// Remove a reference between paths.
    pub fn remove_reference(&self, referrer_path: &str, reference_path: &str) -> Result<()> {
        self.conn.execute(
            r#"
            DELETE FROM Refs
            WHERE referrer = (SELECT id FROM ValidPaths WHERE path = ?1)
              AND reference = (SELECT id FROM ValidPaths WHERE path = ?2)
            "#,
            params![referrer_path, reference_path],
        )?;
        Ok(())
    }

    /// Register a derivation output.
    pub fn register_derivation_output(
        &self,
        drv_path: &str,
        output_id: &str,
        output_path: &str,
    ) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT OR REPLACE INTO DerivationOutputs (drv, id, path)
            SELECT v.id, ?2, ?3
            FROM ValidPaths v
            WHERE v.path = ?1
            "#,
            params![drv_path, output_id, output_path],
        )?;
        Ok(())
    }

    /// Delete a valid path from the database.
    ///
    /// This will cascade-delete associated refs and derivation outputs.
    pub fn invalidate_path(&self, path: &str) -> Result<bool> {
        let rows = self
            .conn
            .execute("DELETE FROM ValidPaths WHERE path = ?1", params![path])?;
        Ok(rows > 0)
    }

    /// Update signatures for a path.
    pub fn update_signatures(&self, path: &str, sigs: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE ValidPaths SET sigs = ?2 WHERE path = ?1",
            params![path, sigs],
        )?;
        Ok(())
    }

    /// Register a realisation (for CA derivations).
    pub fn register_realisation(
        &self,
        drv_path: &str,
        output_name: &str,
        output_path_id: i64,
        signatures: Option<&str>,
    ) -> Result<i64> {
        self.conn.execute(
            r#"
            INSERT INTO Realisations (drvPath, outputName, outputPath, signatures)
            VALUES (?1, ?2, ?3, ?4)
            "#,
            params![drv_path, output_name, output_path_id, signatures],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Add a reference between realisations.
    pub fn add_realisation_reference(&self, referrer_id: i64, reference_id: i64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO RealisationsRefs (referrer, realisationReference) VALUES (?1, ?2)",
            params![referrer_id, reference_id],
        )?;
        Ok(())
    }
}
