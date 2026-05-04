// SPDX-FileCopyrightText: 2025 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! Write operations for the store database.
//!
//! These are primarily used for testing and local store management.

use rusqlite::params;

use harmonia_store_core::store_path::{StoreDir, StorePath};

use crate::connection::StoreDb;
use crate::error::Result;

impl StoreDb {
    /// Register a new valid path.
    ///
    /// Returns the database ID of the new path.
    pub fn register_valid_path(
        &mut self,
        store_dir: &StoreDir,
        path: &StorePath,
        info: &harmonia_store_path_info::UnkeyedValidPathInfo,
    ) -> Result<i64> {
        let full_path = store_dir.display(path).to_string();
        let hash_str = format!("sha256:{:x}", info.nar_hash);
        let deriver_str = info
            .deriver
            .as_ref()
            .map(|d| store_dir.display(d).to_string());
        let sigs_str = if info.signatures.is_empty() {
            None
        } else {
            Some(
                info.signatures
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(" "),
            )
        };
        let ca_str = info.ca.as_ref().map(ToString::to_string);
        let reg_time = info.registration_time.map(|t| t.get()).unwrap_or(0);

        let tx = self.conn.transaction()?;

        tx.execute(
            r#"
            INSERT INTO ValidPaths (path, hash, registrationTime, deriver, narSize, ultimate, sigs, ca)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            "#,
            params![
                full_path,
                hash_str,
                reg_time,
                deriver_str,
                info.nar_size as i64,
                if info.ultimate { 1 } else { 0 },
                sigs_str,
                ca_str,
            ],
        )?;

        let id = tx.last_insert_rowid();

        for reference in &info.references {
            let ref_full = store_dir.display(reference).to_string();
            let ref_id: Option<i64> = tx
                .query_row(
                    "SELECT id FROM ValidPaths WHERE path = ?1",
                    params![ref_full],
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
    pub fn add_reference(
        &self,
        store_dir: &StoreDir,
        referrer: &StorePath,
        reference: &StorePath,
    ) -> Result<()> {
        let referrer_full = store_dir.display(referrer).to_string();
        let reference_full = store_dir.display(reference).to_string();
        self.conn.execute(
            r#"
            INSERT OR REPLACE INTO Refs (referrer, reference)
            SELECT r.id, f.id
            FROM ValidPaths r, ValidPaths f
            WHERE r.path = ?1 AND f.path = ?2
            "#,
            params![referrer_full, reference_full],
        )?;
        Ok(())
    }

    /// Remove a reference between paths.
    pub fn remove_reference(
        &self,
        store_dir: &StoreDir,
        referrer: &StorePath,
        reference: &StorePath,
    ) -> Result<()> {
        let referrer_full = store_dir.display(referrer).to_string();
        let reference_full = store_dir.display(reference).to_string();
        self.conn.execute(
            r#"
            DELETE FROM Refs
            WHERE referrer = (SELECT id FROM ValidPaths WHERE path = ?1)
              AND reference = (SELECT id FROM ValidPaths WHERE path = ?2)
            "#,
            params![referrer_full, reference_full],
        )?;
        Ok(())
    }

    /// Register a derivation output.
    pub fn register_derivation_output(
        &self,
        store_dir: &StoreDir,
        drv_path: &StorePath,
        output_id: &harmonia_store_core::derived_path::OutputName,
        output_path: &StorePath,
    ) -> Result<()> {
        let drv_full = store_dir.display(drv_path).to_string();
        let out_full = store_dir.display(output_path).to_string();
        let output_id_str: &str = output_id.as_ref();
        self.conn.execute(
            r#"
            INSERT OR REPLACE INTO DerivationOutputs (drv, id, path)
            SELECT v.id, ?2, ?3
            FROM ValidPaths v
            WHERE v.path = ?1
            "#,
            params![drv_full, output_id_str, out_full],
        )?;
        Ok(())
    }

    /// Delete a valid path from the database.
    ///
    /// This will cascade-delete associated refs and derivation outputs.
    pub fn invalidate_path(&self, store_dir: &StoreDir, path: &StorePath) -> Result<bool> {
        let full = store_dir.display(path).to_string();
        let rows = self
            .conn
            .execute("DELETE FROM ValidPaths WHERE path = ?1", params![full])?;
        Ok(rows > 0)
    }

    /// Update signatures for a path.
    pub fn update_signatures(
        &self,
        store_dir: &StoreDir,
        path: &StorePath,
        sigs: &str,
    ) -> Result<()> {
        let full = store_dir.display(path).to_string();
        self.conn.execute(
            "UPDATE ValidPaths SET sigs = ?2 WHERE path = ?1",
            params![full, sigs],
        )?;
        Ok(())
    }

    /// Register a realisation (for CA derivations).
    ///
    /// `drvPath` is stored as a base path (matching Nix's format), while
    /// `outputPath` uses the full store dir prefix.
    pub fn register_realisation(
        &self,
        store_dir: &StoreDir,
        realisation: &harmonia_store_core::realisation::Realisation,
    ) -> Result<i64> {
        let drv_path = realisation.key.drv_path.to_string();
        let output_name: &str = realisation.key.output_name.as_ref();
        let output_path = store_dir.display(&realisation.value.out_path).to_string();
        let signatures = if realisation.value.signatures.is_empty() {
            None
        } else {
            Some(
                realisation
                    .value
                    .signatures
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(" "),
            )
        };
        self.conn.execute(
            r#"
            INSERT INTO BuildTraceV3 (drvPath, outputName, outputPath, signatures)
            VALUES (?1, ?2, ?3, ?4)
            "#,
            params![drv_path, output_name, output_path, signatures],
        )?;
        Ok(self.conn.last_insert_rowid())
    }
}
