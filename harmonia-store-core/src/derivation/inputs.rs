use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::derived_path::{OutputName, SingleDerivedPath};
use crate::store_path::{StorePath, StorePathSet};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputInputs {
    /// The specific outputs needed from this derivation
    #[serde(default)]
    pub outputs: BTreeSet<OutputName>,
    /// Dynamic outputs (experimental feature)
    #[serde(default)]
    pub dynamic_outputs: BTreeMap<OutputName, OutputInputs>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct DerivationInputs {
    #[serde(default)]
    pub srcs: StorePathSet,
    /// `SingleDerivedPath::Built` inputs as a trie-like structure.
    #[serde(default)]
    pub drvs: BTreeMap<StorePath, OutputInputs>,
}

impl From<&DerivationInputs> for BTreeSet<SingleDerivedPath> {
    fn from(inputs: &DerivationInputs) -> Self {
        let mut paths = Self::default();

        // Add all source paths as Opaque
        for src in &inputs.srcs {
            paths.insert(SingleDerivedPath::Opaque(src.clone()));
        }

        // Add all derivation inputs as Built paths
        // Use a recursive closure to expand the trie structure into paths
        fn expand_outputs(
            paths: &mut BTreeSet<SingleDerivedPath>,
            drv_path: Arc<SingleDerivedPath>,
            output_inputs: &OutputInputs,
        ) {
            // Add direct outputs
            for output in &output_inputs.outputs {
                paths.insert(SingleDerivedPath::Built {
                    drv_path: drv_path.clone(),
                    output: output.clone(),
                });
            }

            // Add dynamic outputs recursively
            for (output_name, nested_outputs) in &output_inputs.dynamic_outputs {
                expand_outputs(
                    paths,
                    Arc::new(SingleDerivedPath::Built {
                        drv_path: drv_path.clone(),
                        output: output_name.clone(),
                    }),
                    nested_outputs,
                );
            }
        }

        for (drv_path, output_inputs) in &inputs.drvs {
            expand_outputs(
                &mut paths,
                Arc::new(SingleDerivedPath::Opaque(drv_path.clone())),
                output_inputs,
            );
        }

        paths
    }
}

impl From<&BTreeSet<SingleDerivedPath>> for DerivationInputs {
    fn from(paths: &BTreeSet<SingleDerivedPath>) -> Self {
        let mut result = Self::default();

        // Use a recursive closure to navigate the trie, similar to Nix's DerivedPathMap::ensureSlot
        fn ensure_slot<'a>(
            drvs: &'a mut BTreeMap<StorePath, OutputInputs>,
            k: &SingleDerivedPath,
        ) -> &'a mut OutputInputs {
            match k {
                SingleDerivedPath::Opaque(store_path) => {
                    // Base case: will not overwrite if already there
                    drvs.entry(store_path.clone()).or_default()
                }
                SingleDerivedPath::Built { drv_path, output } => {
                    // Recursive case: navigate to the parent node first
                    let node = ensure_slot(drvs, drv_path.as_ref());
                    // Then navigate into its childMap (dynamic_outputs)
                    node.dynamic_outputs.entry(output.clone()).or_default()
                }
            }
        }

        for path in paths {
            match path {
                SingleDerivedPath::Opaque(store_path) => {
                    result.srcs.insert(store_path.clone());
                }
                SingleDerivedPath::Built { drv_path, output } => {
                    // Get the slot for this path and add the output
                    ensure_slot(&mut result.drvs, drv_path.as_ref())
                        .outputs
                        .insert(output.clone());
                }
            }
        }

        result
    }
}
