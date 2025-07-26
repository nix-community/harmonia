use harmonia_store_core::StorePath;
use std::collections::BTreeSet;

/// Information about a substitutable path
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubstitutablePathInfo {
    /// The deriver of this substitutable path, if known
    pub deriver: Option<StorePath>,
    /// The references of this substitutable path
    pub references: BTreeSet<StorePath>,
    /// The size of the substitutable path when downloaded
    pub download_size: u64,
    /// The NAR size of the substitutable path  
    pub nar_size: u64,
}

/// A map of store paths to their substitutable path info
pub type SubstitutablePathInfos = std::collections::BTreeMap<StorePath, SubstitutablePathInfo>;
