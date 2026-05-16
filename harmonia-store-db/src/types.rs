// SPDX-FileCopyrightText: 2025 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! Database row types for Nix store metadata.

/// SQLite row ID for a valid path.
pub type ValidPathId = i64;

/// SQLite row ID for a realisation.
pub type RealisationId = i64;

/// Information about a valid store path, with its database row ID.
///
/// Wraps `harmonia_store_path_info::UnkeyedValidPathInfo` with the SQLite row
/// ID and the store path (the key).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidPathInfo {
    /// Database row ID
    pub id: ValidPathId,
    /// The store path.
    pub path: harmonia_store_path::StorePath,
    /// Metadata about the path.
    pub info: harmonia_store_path_info::UnkeyedValidPathInfo,
}

/// A reference between two store paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathRef {
    /// ID of the path that has the reference
    pub referrer_id: ValidPathId,
    /// ID of the path being referenced
    pub reference_id: ValidPathId,
}

/// A derivation output mapping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DerivationOutput {
    /// ID of the derivation path
    pub drv_id: ValidPathId,
    /// Symbolic output name (usually "out")
    pub output_id: harmonia_store_core::derived_path::OutputName,
    /// Store path of the output
    pub path: harmonia_store_path::StorePath,
}

/// A content-addressed derivation realisation (`BuildTraceV3` row).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Realisation {
    /// Database row ID
    pub id: RealisationId,
    /// The realisation data.
    pub realisation: harmonia_store_core::realisation::Realisation,
}
