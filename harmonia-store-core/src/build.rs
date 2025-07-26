use crate::{Hash, StorePath};
use std::collections::BTreeMap;

/// Build modes for derivation building
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u64)]
#[derive(Default)]
pub enum BuildMode {
    /// Normal build mode
    #[default]
    Normal = 0,
    /// Repair mode - rebuild even if already valid
    Repair = 1,
    /// Check mode - verify build reproducibility
    Check = 2,
}

/// Build status codes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u64)]
pub enum BuildStatus {
    /// Build succeeded
    Built = 0,
    /// Build was substituted
    Substituted = 1,
    /// Already valid, no build needed
    AlreadyValid = 2,
    /// Permanent build failure
    PermanentFailure = 3,
    /// Input derivation failed to build
    InputRejected = 4,
    /// Output rejected by output validator
    OutputRejected = 5,
    /// Transient failure (e.g. network issue)
    TransientFailure = 6,
    /// Build timed out
    TimedOut = 7,
    /// Build failed due to host system issue
    MiscFailure = 8,
    /// Dependency failed to build
    DependencyFailed = 9,
    /// Build log limit was exceeded
    LogLimitExceeded = 10,
    /// Build hook declined the build
    NotDeterministic = 11,
    /// Resource exhaustion
    ResolvesToAlreadyValid = 12,
    /// Build was postponed (internal)
    NoSubstituters = 13,
}

/// Result of building a single derivation
#[derive(Debug, Clone)]
pub struct BuildResult {
    /// Status of the build
    pub status: BuildStatus,
    /// Error message if build failed
    pub error_msg: Option<Vec<u8>>,
    /// Build log last lines (protocol < 1.29)
    pub log_lines: Vec<Vec<u8>>,
    /// Number of times the build was attempted
    pub times_built: u32,
    /// Whether the build is non-deterministic
    pub is_non_deterministic: bool,
    /// Start time of the build (Unix timestamp)
    pub start_time: u64,
    /// Stop time of the build (Unix timestamp)  
    pub stop_time: u64,
    /// Status of each built output (protocol >= 1.28)
    pub built_outputs: BTreeMap<Vec<u8>, DrvOutputResult>,
}

/// Result for a single derivation output
#[derive(Debug, Clone)]
pub struct DrvOutputResult {
    /// Path where output was realized
    pub path: StorePath,
    /// Hash of the output (if known)
    pub hash: Option<Hash>,
}
