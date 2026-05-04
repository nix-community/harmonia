// SPDX-FileCopyrightText: 2025 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! Read query operations for the store database.

use std::collections::BTreeSet;
use std::num::NonZero;

use rusqlite::params;

use harmonia_store_core::store_path::{StoreDir, StorePath, StorePathHash};

use crate::connection::StoreDb;
use crate::error::Result;
use crate::types::{DerivationOutput, Realisation, ValidPathInfo};

fn parse_from_sql<T: std::str::FromStr>(
    col: usize,
    s: &str,
) -> std::result::Result<T, rusqlite::Error>
where
    T::Err: std::error::Error + Send + Sync + 'static,
{
    s.parse().map_err(|e: T::Err| {
        rusqlite::Error::FromSqlConversionFailure(col, rusqlite::types::Type::Text, Box::new(e))
    })
}

/// Build a `ValidPathInfo` from a row, parsing proper types from raw SQL strings.
fn valid_path_info_from_row(
    store_dir: &StoreDir,
    row: &rusqlite::Row<'_>,
) -> std::result::Result<ValidPathInfo, rusqlite::Error> {
    let id: i64 = row.get(0)?;
    let path_str: String = row.get(1)?;
    let hash_str: String = row.get(2)?;
    let reg_time: i64 = row.get(3)?;
    let deriver_str: Option<String> = row.get(4)?;
    let nar_size: Option<i64> = row.get(5)?;
    let ultimate: Option<i32> = row.get(6)?;
    let sigs_str: Option<String> = row.get(7)?;
    let ca_str: Option<String> = row.get(8)?;

    let path = store_dir.parse(&path_str).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(e))
    })?;

    let nar_hash = {
        let parsed = parse_from_sql::<
            harmonia_utils_hash::fmt::Any<harmonia_store_path_info::NarHash>,
        >(2, &hash_str)?;
        parsed.into_hash()
    };

    let deriver = deriver_str
        .map(|s| store_dir.parse(&s))
        .transpose()
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(4, rusqlite::types::Type::Text, Box::new(e))
        })?;

    let signatures = sigs_str
        .map(|s| {
            s.split_whitespace()
                .filter_map(|sig| sig.parse().ok())
                .collect()
        })
        .unwrap_or_default();

    let ca = ca_str
        .map(|s| parse_from_sql::<harmonia_store_core::store_path::ContentAddress>(8, &s))
        .transpose()?;

    Ok(ValidPathInfo {
        id,
        path,
        info: harmonia_store_path_info::UnkeyedValidPathInfo {
            deriver,
            nar_hash,
            references: BTreeSet::new(), // filled in by caller
            registration_time: NonZero::new(reg_time),
            nar_size: nar_size.map(|n| n as u64).unwrap_or(0),
            ultimate: ultimate.unwrap_or(0) != 0,
            signatures,
            ca,
            store_dir: store_dir.clone(),
        },
    })
}

impl StoreDb {
    /// Query path info by full store path.
    ///
    /// Returns `None` if the path is not in the database.
    pub fn query_path_info(
        &self,
        store_dir: &StoreDir,
        path: &StorePath,
    ) -> Result<Option<ValidPathInfo>> {
        let full = store_dir.display(path).to_string();
        let mut stmt = self.conn.prepare_cached(
            r#"
            SELECT id, path, hash, registrationTime, deriver, narSize, ultimate, sigs, ca
            FROM ValidPaths
            WHERE path = ?1
            "#,
        )?;

        let info = stmt.query_row(params![full], |row| {
            valid_path_info_from_row(store_dir, row)
        });

        match info {
            Ok(mut info) => {
                info.info.references = self.query_reference_paths_by_id(store_dir, info.id)?;
                Ok(Some(info))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Query path info by database ID.
    pub fn query_path_info_by_id(
        &self,
        store_dir: &StoreDir,
        id: i64,
    ) -> Result<Option<ValidPathInfo>> {
        let mut stmt = self.conn.prepare_cached(
            r#"
            SELECT id, path, hash, registrationTime, deriver, narSize, ultimate, sigs, ca
            FROM ValidPaths
            WHERE id = ?1
            "#,
        )?;

        let info = stmt.query_row(params![id], |row| valid_path_info_from_row(store_dir, row));

        match info {
            Ok(mut info) => {
                info.info.references = self.query_reference_paths_by_id(store_dir, info.id)?;
                Ok(Some(info))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Look up full path info by the store path's 32-char hash part.
    pub fn query_path_info_by_hash_part(
        &self,
        store_dir: &StoreDir,
        hash_part: &StorePathHash,
    ) -> Result<Option<ValidPathInfo>> {
        let prefix = format!("{store_dir}/{hash_part}");

        let mut stmt = self.conn.prepare_cached(
            r#"
            SELECT id, path, hash, registrationTime, deriver, narSize, ultimate, sigs, ca
            FROM ValidPaths
            WHERE path >= ?1 LIMIT 1
            "#,
        )?;

        let info = stmt.query_row(params![&prefix], |row| {
            valid_path_info_from_row(store_dir, row)
        });

        match info {
            Ok(mut info) => {
                let full_path = store_dir.display(&info.path).to_string();
                if full_path.starts_with(&prefix) {
                    info.info.references = self.query_reference_paths_by_id(store_dir, info.id)?;
                    Ok(Some(info))
                } else {
                    Ok(None)
                }
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Look up a store path by its hash part (the 32-character prefix).
    pub fn query_path_from_hash_part(
        &self,
        store_dir: &StoreDir,
        hash_part: &StorePathHash,
    ) -> Result<Option<StorePath>> {
        let prefix = format!("{store_dir}/{hash_part}");

        let mut stmt = self.conn.prepare_cached(
            r#"
            SELECT path FROM ValidPaths WHERE path >= ?1 LIMIT 1
            "#,
        )?;

        let result: Option<String> = stmt.query_row(params![&prefix], |row| row.get(0)).ok();

        match result {
            Some(path) if path.starts_with(&prefix) => Ok(store_dir.parse(&path).ok()),
            _ => Ok(None),
        }
    }

    /// Check if a store path is valid (exists in the database).
    pub fn is_valid_path(&self, store_dir: &StoreDir, path: &StorePath) -> Result<bool> {
        let full = store_dir.display(path).to_string();
        let mut stmt = self.conn.prepare_cached(
            r#"
            SELECT 1 FROM ValidPaths WHERE path = ?1 LIMIT 1
            "#,
        )?;

        let exists = stmt.query_row(params![full], |_| Ok(())).is_ok();
        Ok(exists)
    }

    /// Get all paths referenced by a given path.
    pub fn query_references(
        &self,
        store_dir: &StoreDir,
        path: &StorePath,
    ) -> Result<BTreeSet<StorePath>> {
        let full = store_dir.display(path).to_string();
        let mut stmt = self.conn.prepare_cached(
            r#"
            SELECT v.path
            FROM Refs r
            JOIN ValidPaths v ON r.reference = v.id
            WHERE r.referrer = (SELECT id FROM ValidPaths WHERE path = ?1)
            "#,
        )?;

        let mut refs = BTreeSet::new();
        let mut rows = stmt.query(params![full])?;
        while let Some(row) = rows.next()? {
            let path_str: String = row.get(0)?;
            if let Ok(sp) = store_dir.parse(&path_str) {
                refs.insert(sp);
            }
        }
        Ok(refs)
    }

    /// Get all paths referenced by a given path (by ID).
    fn query_reference_paths_by_id(
        &self,
        store_dir: &StoreDir,
        id: crate::types::ValidPathId,
    ) -> Result<BTreeSet<StorePath>> {
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
            let path_str: String = row.get(0)?;
            if let Ok(sp) = store_dir.parse(&path_str) {
                refs.insert(sp);
            }
        }
        Ok(refs)
    }

    /// Get all paths that reference a given path (reverse dependencies).
    pub fn query_referrers(
        &self,
        store_dir: &StoreDir,
        path: &StorePath,
    ) -> Result<BTreeSet<StorePath>> {
        let full = store_dir.display(path).to_string();
        let mut stmt = self.conn.prepare_cached(
            r#"
            SELECT v.path
            FROM Refs r
            JOIN ValidPaths v ON r.referrer = v.id
            WHERE r.reference = (SELECT id FROM ValidPaths WHERE path = ?1)
            "#,
        )?;

        let mut refs = BTreeSet::new();
        let mut rows = stmt.query(params![full])?;
        while let Some(row) = rows.next()? {
            let path_str: String = row.get(0)?;
            if let Ok(sp) = store_dir.parse(&path_str) {
                refs.insert(sp);
            }
        }
        Ok(refs)
    }

    /// Get all derivations that produced a given output path.
    pub fn query_valid_derivers(
        &self,
        store_dir: &StoreDir,
        output_path: &StorePath,
    ) -> Result<Vec<StorePath>> {
        let full = store_dir.display(output_path).to_string();
        let mut stmt = self.conn.prepare_cached(
            r#"
            SELECT v.path
            FROM DerivationOutputs d
            JOIN ValidPaths v ON d.drv = v.id
            WHERE d.path = ?1
            "#,
        )?;

        let mut derivers = Vec::new();
        let mut rows = stmt.query(params![full])?;
        while let Some(row) = rows.next()? {
            let path_str: String = row.get(0)?;
            if let Ok(sp) = store_dir.parse(&path_str) {
                derivers.push(sp);
            }
        }
        Ok(derivers)
    }

    /// Get all outputs of a derivation.
    pub fn query_derivation_outputs(
        &self,
        store_dir: &StoreDir,
        drv_path: &StorePath,
    ) -> Result<Vec<DerivationOutput>> {
        let full = store_dir.display(drv_path).to_string();
        let mut stmt = self.conn.prepare_cached(
            r#"
            SELECT d.drv, d.id, d.path
            FROM DerivationOutputs d
            JOIN ValidPaths v ON d.drv = v.id
            WHERE v.path = ?1
            "#,
        )?;

        let mut outputs = Vec::new();
        let mut rows = stmt.query(params![full])?;
        while let Some(row) = rows.next()? {
            let output_id_str: String = row.get(1)?;
            let path_str: String = row.get(2)?;
            let path = store_dir.parse(&path_str).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    2,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?;
            outputs.push(DerivationOutput {
                drv_id: row.get(0)?,
                output_id: parse_from_sql(1, &output_id_str)?,
                path,
            });
        }
        Ok(outputs)
    }

    /// Get all valid paths in the database.
    ///
    /// Warning: This can be slow for large stores!
    pub fn query_all_valid_paths(&self, store_dir: &StoreDir) -> Result<Vec<StorePath>> {
        let mut stmt = self.conn.prepare_cached("SELECT path FROM ValidPaths")?;

        let mut paths = Vec::new();
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let path_str: String = row.get(0)?;
            if let Ok(sp) = store_dir.parse(&path_str) {
                paths.push(sp);
            }
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
    ///
    /// Nix stores full paths (e.g. `/nix/store/hash-name`) in `BuildTraceV3`,
    /// so we need the store dir to build the WHERE value and parse results.
    pub fn query_realisation(
        &self,
        store_dir: &StoreDir,
        drv_path: &StorePath,
        output_name: &harmonia_store_core::derived_path::OutputName,
    ) -> Result<Option<Realisation>> {
        use harmonia_store_core::realisation;

        let mut stmt = self.conn.prepare_cached(
            r#"
            SELECT id, drvPath, outputName, outputPath, signatures
            FROM BuildTraceV3
            WHERE drvPath = ?1 AND outputName = ?2
            "#,
        )?;

        let drv_path_s = drv_path.to_string();
        let output_name_s: &str = output_name.as_ref();
        let result = stmt.query_row(params![drv_path_s, output_name_s], |row| {
            let id: i64 = row.get(0)?;
            let drv_path_str: String = row.get(1)?;
            let output_name_str: String = row.get(2)?;
            let output_path_str: String = row.get(3)?;
            let sigs_str: Option<String> = row.get(4)?;

            let drv_path = drv_path_str.parse().map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    1,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?;
            let output_name = output_name_str.parse().map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    2,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?;
            let out_path = store_dir.parse(&output_path_str).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    3,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?;
            let signatures = sigs_str
                .map(|s| {
                    s.split_whitespace()
                        .filter_map(|sig| sig.parse().ok())
                        .collect()
                })
                .unwrap_or_default();

            Ok(Realisation {
                id,
                realisation: realisation::Realisation {
                    key: realisation::DrvOutput {
                        drv_path,
                        output_name,
                    },
                    value: realisation::UnkeyedRealisation {
                        out_path,
                        signatures,
                    },
                },
            })
        });

        match result {
            Ok(r) => Ok(Some(r)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}
