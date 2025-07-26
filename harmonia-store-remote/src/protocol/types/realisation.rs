use harmonia_store_core::{StorePath, NarSignature};
use std::collections::BTreeSet;

/// Represents a derivation output identifier
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DrvOutputId {
    /// The hash part of the derivation  
    pub drv_hash: Vec<u8>,
    /// The output name
    pub output_name: Vec<u8>,
}

/// Represents a realisation of a derivation output
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Realisation {
    /// The derivation output identifier
    pub id: DrvOutputId,
    /// The output path that was realised
    pub out_path: StorePath,
    /// Signatures on this realisation
    pub signatures: BTreeSet<NarSignature>,
}

/// Settings for the daemon
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonSettings {
    /// Whether to keep going on build failures
    pub keep_going: bool,
    /// Whether to keep build failures  
    pub keep_failed: bool,
    /// Whether to try fallback on build failures
    pub try_fallback: bool,
    /// Verbosity level
    pub verbosity: u64,
    /// Maximum build jobs
    pub max_build_jobs: u64,
    /// Maximum silent time in seconds
    pub max_silent_time: u64,
    /// Use build hook
    pub use_build_hook: bool,
    /// Build hook program path
    pub build_hook: Option<Vec<u8>>,
    /// Build cores
    pub build_cores: u64,
    /// Use substitutes
    pub use_substitutes: bool,
    /// Substitute URLs
    pub substitute_urls: Vec<Vec<u8>>,
}