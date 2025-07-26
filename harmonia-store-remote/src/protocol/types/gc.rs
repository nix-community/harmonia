use harmonia_store_core::StorePath;
use std::collections::BTreeSet;

/// Garbage collection action to perform
#[repr(u64)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GCAction {
    /// Return the set of paths reachable from roots
    ReturnLive = 0,
    /// Return the set of paths not reachable from roots
    ReturnDead = 1,
    /// Delete paths not reachable from roots
    DeleteDead = 2,
    /// Delete specific paths
    DeleteSpecific = 3,
}

impl GCAction {
    /// Convert from u64 representation
    pub fn from_u64(value: u64) -> Option<Self> {
        match value {
            0 => Some(Self::ReturnLive),
            1 => Some(Self::ReturnDead),
            2 => Some(Self::DeleteDead),
            3 => Some(Self::DeleteSpecific),
            _ => None,
        }
    }

    /// Get the u64 representation
    pub fn as_u64(&self) -> u64 {
        *self as u64
    }
}

/// Options for garbage collection
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GCOptions {
    /// The GC operation to perform
    pub operation: GCAction,

    /// Whether to ignore liveness (dangerous!)
    pub ignore_liveness: bool,

    /// Specific paths to delete (only used with DeleteSpecific)
    pub paths_to_delete: BTreeSet<StorePath>,

    /// Maximum number of bytes to free (0 means no limit)
    pub max_freed: u64,
}

impl GCOptions {
    /// Create options for returning live paths
    pub fn return_live() -> Self {
        Self {
            operation: GCAction::ReturnLive,
            ignore_liveness: false,
            paths_to_delete: BTreeSet::new(),
            max_freed: 0,
        }
    }

    /// Create options for returning dead paths
    pub fn return_dead() -> Self {
        Self {
            operation: GCAction::ReturnDead,
            ignore_liveness: false,
            paths_to_delete: BTreeSet::new(),
            max_freed: 0,
        }
    }

    /// Create options for deleting dead paths
    pub fn delete_dead(max_freed: u64) -> Self {
        Self {
            operation: GCAction::DeleteDead,
            ignore_liveness: false,
            paths_to_delete: BTreeSet::new(),
            max_freed,
        }
    }

    /// Create options for deleting specific paths
    pub fn delete_specific(paths: BTreeSet<StorePath>) -> Self {
        Self {
            operation: GCAction::DeleteSpecific,
            ignore_liveness: false,
            paths_to_delete: paths,
            max_freed: 0,
        }
    }
}

/// Result of garbage collection
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GCResult {
    /// Paths that were deleted
    pub deleted_paths: BTreeSet<StorePath>,

    /// Total bytes freed
    pub bytes_freed: u64,
}

impl GCResult {
    /// Create a new empty GC result
    pub fn new() -> Self {
        Self {
            deleted_paths: BTreeSet::new(),
            bytes_freed: 0,
        }
    }

    /// Get the number of deleted paths
    pub fn deleted_count(&self) -> usize {
        self.deleted_paths.len()
    }
}

impl Default for GCResult {
    fn default() -> Self {
        Self::new()
    }
}

/// A garbage collection root
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GCRoot {
    /// Root path is censored (e.g., for security reasons)
    Censored,
    /// The actual root path
    Path(StorePath),
}

impl GCRoot {
    /// Returns true if this root is censored
    pub fn is_censored(&self) -> bool {
        matches!(self, Self::Censored)
    }

    /// Get the store path if not censored
    pub fn path(&self) -> Option<&StorePath> {
        match self {
            Self::Censored => None,
            Self::Path(path) => Some(path),
        }
    }
}
