use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::store_path::{StorePath, StorePathSet};

/// A single derivation input specifying which outputs are needed
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputInputs {
    /// The specific outputs needed from this derivation
    pub outputs: BTreeSet<String>,
    /// Dynamic outputs (experimental feature)
    #[serde(default)]
    pub dynamic_outputs: BTreeMap<String, OutputInputs>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DerivationInputs {
    #[serde(default)]
    pub srcs: StorePathSet,
    #[serde(default)]
    pub drvs: BTreeMap<StorePath, OutputInputs>,
}
