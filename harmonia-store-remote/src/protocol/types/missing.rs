use harmonia_store_core::StorePath;
use std::collections::BTreeSet;

/// Result of querying what paths are missing and need to be built/substituted
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Missing {
    /// Derivations that will be built
    pub will_build: BTreeSet<StorePath>,

    /// Store paths that will be substituted (downloaded)
    pub will_substitute: BTreeSet<StorePath>,

    /// Paths that are unknown (not in store and no substitutes available)
    pub unknown_paths: BTreeSet<StorePath>,

    /// Total download size for substitutions (in bytes)
    pub download_size: u64,

    /// Total NAR size of substitutions (in bytes)
    pub nar_size: u64,
}

impl Missing {
    /// Create a new empty Missing result
    pub fn new() -> Self {
        Self {
            will_build: BTreeSet::new(),
            will_substitute: BTreeSet::new(),
            unknown_paths: BTreeSet::new(),
            download_size: 0,
            nar_size: 0,
        }
    }

    /// Returns true if nothing is missing (no builds or substitutions needed)
    pub fn is_empty(&self) -> bool {
        self.will_build.is_empty()
            && self.will_substitute.is_empty()
            && self.unknown_paths.is_empty()
    }

    /// Get the total number of missing paths
    pub fn total_missing(&self) -> usize {
        self.will_build.len() + self.will_substitute.len() + self.unknown_paths.len()
    }

    /// Get the total number of paths that will be made available
    pub fn total_available(&self) -> usize {
        self.will_build.len() + self.will_substitute.len()
    }
}

impl Default for Missing {
    fn default() -> Self {
        Self::new()
    }
}
