// SPDX-FileCopyrightText: 2025 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! Database row types for Nix store metadata.

use std::collections::BTreeSet;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Information about a valid store path.
///
/// This represents a row from the ValidPaths table with its references.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidPathInfo {
    /// Database row ID
    pub id: i64,
    /// Full store path (e.g., /nix/store/xxx-name)
    pub path: String,
    /// Base16-encoded content hash
    pub hash: String,
    /// When this path was registered (Unix timestamp)
    pub registration_time: SystemTime,
    /// Store path of the derivation that produced this (if any)
    pub deriver: Option<String>,
    /// Size of the NAR serialization
    pub nar_size: Option<u64>,
    /// Whether this is an "ultimate" path (built locally, not substituted)
    pub ultimate: bool,
    /// Space-separated cryptographic signatures
    pub sigs: Option<String>,
    /// Content address assertion (if content-addressed)
    pub ca: Option<String>,
    /// Store paths that this path references (runtime dependencies)
    pub references: BTreeSet<String>,
}

impl ValidPathInfo {
    /// Parse signatures from space-separated string.
    pub fn signatures(&self) -> Vec<&str> {
        self.sigs
            .as_deref()
            .map(|s| s.split_whitespace().collect())
            .unwrap_or_default()
    }

    /// Check if this path has any signatures.
    pub fn is_signed(&self) -> bool {
        self.sigs.as_ref().is_some_and(|s| !s.trim().is_empty())
    }
}

/// A reference between two store paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathRef {
    /// ID of the path that has the reference
    pub referrer_id: i64,
    /// ID of the path being referenced
    pub reference_id: i64,
}

/// A derivation output mapping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DerivationOutput {
    /// ID of the derivation path
    pub drv_id: i64,
    /// Symbolic output name (usually "out")
    pub output_id: String,
    /// Store path of the output
    pub path: String,
}

/// A content-addressed derivation realisation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Realisation {
    /// Database row ID
    pub id: i64,
    /// Path to the derivation
    pub drv_path: String,
    /// Output name (usually "out")
    pub output_name: String,
    /// ID of the output path in ValidPaths
    pub output_path_id: i64,
    /// Space-separated signatures
    pub signatures: Option<String>,
}

/// A reference between realisations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealisationRef {
    /// ID of the realisation that has the reference
    pub referrer_id: i64,
    /// ID of the realisation being referenced
    pub reference_id: Option<i64>,
}

/// Convert Unix timestamp to SystemTime.
pub(crate) fn unix_to_system_time(timestamp: i64) -> SystemTime {
    if timestamp >= 0 {
        UNIX_EPOCH + Duration::from_secs(timestamp as u64)
    } else {
        UNIX_EPOCH - Duration::from_secs((-timestamp) as u64)
    }
}

/// Convert SystemTime to Unix timestamp.
pub(crate) fn system_time_to_unix(time: SystemTime) -> i64 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_secs() as i64,
        Err(e) => -(e.duration().as_secs() as i64),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unix_time_roundtrip() {
        let now = SystemTime::now();
        let unix = system_time_to_unix(now);
        let back = unix_to_system_time(unix);
        // Allow 1 second tolerance due to subsecond truncation
        let diff = now.duration_since(back).unwrap_or_default();
        assert!(diff.as_secs() <= 1);
    }

    #[test]
    fn test_signatures_parsing() {
        let info = ValidPathInfo {
            id: 1,
            path: "/nix/store/test".into(),
            hash: "abc".into(),
            registration_time: UNIX_EPOCH,
            deriver: None,
            nar_size: None,
            ultimate: false,
            sigs: Some("cache.example.com:abc123 other:def456".into()),
            ca: None,
            references: BTreeSet::new(),
        };

        let sigs = info.signatures();
        assert_eq!(sigs.len(), 2);
        assert_eq!(sigs[0], "cache.example.com:abc123");
        assert_eq!(sigs[1], "other:def456");
        assert!(info.is_signed());
    }

    #[test]
    fn test_no_signatures() {
        let info = ValidPathInfo {
            id: 1,
            path: "/nix/store/test".into(),
            hash: "abc".into(),
            registration_time: UNIX_EPOCH,
            deriver: None,
            nar_size: None,
            ultimate: false,
            sigs: None,
            ca: None,
            references: BTreeSet::new(),
        };

        assert!(info.signatures().is_empty());
        assert!(!info.is_signed());
    }
}
