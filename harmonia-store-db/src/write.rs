// SPDX-FileCopyrightText: 2025 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! Write operations for the store database.
//!
//! These are primarily used for testing and local store management.

use rusqlite::params;
use tracing::info;

use harmonia_store_path::{StoreDir, StorePath};

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
        output_id: &harmonia_store_derivation::derived_path::OutputName,
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
        realisation: &harmonia_store_derivation::realisation::Realisation,
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

    /// Remove paths by their `ValidPaths.id` in a single transaction.
    ///
    /// Intended for bulk garbage collection. The ids come from
    /// [`StoreGraph::db_id`](crate::StoreGraph::db_id). Reference edges
    /// among the dead set are removed first, because cycles between dead paths would
    /// otherwise violate the `ON DELETE RESTRICT` constraint on
    /// `Refs.reference` (Nix avoids it by deleting one path at a time in
    /// topological order).
    ///
    /// The caller must include every referrer: if some path outside `ids`
    /// still references a path inside `ids`, the referenced path is deleted
    /// anyway and the surviving referrer is left with a missing dependency.
    /// Nix rejects such a deletion, this function does not detect it.
    ///
    /// ```
    /// # fn main() -> harmonia_store_db::Result<()> {
    /// use harmonia_store_db::{GraphOptions, StoreDb};
    /// use harmonia_store_path::StoreDir;
    ///
    /// let db = StoreDb::open_memory()?;
    /// let graph = db.load_graph(&StoreDir::default(), &GraphOptions::default())?;
    /// let dead: Vec<i64> = graph.nodes().map(|n| graph.db_id(n)).collect();
    /// db.invalidate_ids(dead)?; // everything is dead
    /// # Ok(())
    /// # }
    /// ```
    pub fn invalidate_ids(&self, ids: impl IntoIterator<Item = i64>) -> Result<()> {
        self.conn.execute_batch("BEGIN")?;
        let result = self.invalidate_ids_in_txn(ids);
        match result {
            Ok(()) => {
                self.conn.execute_batch("COMMIT")?;
                // Truncate the WAL so its disk use doesn't accumulate across
                // chunks. On a full disk an unbounded WAL would abort a later
                // chunk. Best effort: a blocked checkpoint leaves the WAL for
                // the next one.
                let _ = self.conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)");
                Ok(())
            }
            Err(e) => {
                self.conn.execute_batch("ROLLBACK").ok();
                Err(e)
            }
        }
    }

    fn invalidate_ids_in_txn(&self, ids: impl IntoIterator<Item = i64>) -> Result<()> {
        // A temp table lets the Refs of all dead paths be batch-deleted in
        // two statements instead of one subquery per path.
        self.conn
            .execute_batch("CREATE TEMP TABLE IF NOT EXISTS DeadPaths (id INTEGER PRIMARY KEY)")?;
        self.conn.execute_batch("DELETE FROM DeadPaths")?;
        {
            let mut ins = self.conn.prepare("INSERT INTO DeadPaths VALUES (?)")?;
            for id in ids {
                ins.execute([id])?;
            }
        }
        self.conn.execute_batch(
            "DELETE FROM Refs WHERE referrer IN (SELECT id FROM DeadPaths) \
             OR reference IN (SELECT id FROM DeadPaths)",
        )?;
        // Same cycle problem for the CA realisations of Nix <= 2.34:
        // RealisationsRefs.realisationReference is ON DELETE RESTRICT.
        if crate::graph::has_table(&self.conn, "Realisations")? {
            self.conn.execute_batch(
                "DELETE FROM RealisationsRefs WHERE referrer IN \
                     (SELECT id FROM Realisations WHERE outputPath IN \
                         (SELECT id FROM DeadPaths)) \
                 OR realisationReference IN \
                     (SELECT id FROM Realisations WHERE outputPath IN \
                         (SELECT id FROM DeadPaths)); \
                 DELETE FROM Realisations WHERE outputPath IN \
                     (SELECT id FROM DeadPaths)",
            )?;
        }
        self.conn
            .execute_batch("DELETE FROM ValidPaths WHERE id IN (SELECT id FROM DeadPaths)")?;
        Ok(())
    }

    /// Run `VACUUM` if enough of the database file consists of free pages
    /// to be worth a full rewrite: at least a quarter of the file and at
    /// least 64 pages. Returns whether a vacuum ran.
    ///
    /// The caller must ensure no concurrent writer (e.g. by holding the
    /// GC lock). Concurrent readers are fine in WAL mode.
    pub fn maybe_vacuum(&self) -> Result<bool> {
        let freelist: i64 = self
            .conn
            .query_row("PRAGMA freelist_count", [], |r| r.get(0))?;
        let pages: i64 = self.conn.query_row("PRAGMA page_count", [], |r| r.get(0))?;
        if !vacuum_worthwhile(freelist, pages) {
            return Ok(false);
        }
        info!("vacuuming database ({freelist} of {pages} pages free)...");
        let start = std::time::Instant::now();
        self.conn.execute_batch("VACUUM")?;
        let _ = self.conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)");
        info!("vacuum done in {:.1}s", start.elapsed().as_secs_f64());
        Ok(true)
    }
}

/// At least a quarter of the file and at least 64 pages must be free.
fn vacuum_worthwhile(freelist: i64, pages: i64) -> bool {
    freelist >= 64 && freelist * 4 >= pages
}

#[cfg(test)]
mod tests {
    use super::vacuum_worthwhile;

    #[test]
    fn vacuum_thresholds() {
        assert!(!vacuum_worthwhile(63, 1), "below absolute minimum");
        assert!(vacuum_worthwhile(64, 256), "exactly a quarter free");
        assert!(!vacuum_worthwhile(64, 257), "just under a quarter free");
        assert!(vacuum_worthwhile(1000, 1000), "mostly free");
    }
}
