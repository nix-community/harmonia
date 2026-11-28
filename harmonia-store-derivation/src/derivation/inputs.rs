use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::derived_path::{OutputName, SingleDerivedPath};
use harmonia_store_path::{StorePath, StorePathSet};

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

/// The split-by-kind ("full") input representation used by the on-disk
/// and wire formats: sources vs. input derivations, with the trie
/// structure for dynamic outputs.
///
/// `Key` is how an input derivation is identified — normally a
/// [`StorePath`], but the intermediate derivation used to compute the
/// hash modulo keys input derivations by their own hash modulo instead.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(bound(
    serialize = "Key: Serialize",
    deserialize = "Key: Deserialize<'de> + Ord"
))]
pub struct DerivationInputs<Key = StorePath> {
    #[serde(default)]
    pub srcs: StorePathSet,
    /// `SingleDerivedPath::Built` inputs as a trie-like structure.
    #[serde(default)]
    pub drvs: BTreeMap<Key, OutputInputs>,
}

// Manual impl: a derived `Default` would spuriously require
// `Key: Default` (`BTreeMap` is unconditionally `Default`).
impl<Key> Default for DerivationInputs<Key> {
    fn default() -> Self {
        DerivationInputs {
            srcs: StorePathSet::default(),
            drvs: BTreeMap::new(),
        }
    }
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

impl From<&StorePathSet> for DerivationInputs {
    fn from(srcs: &StorePathSet) -> Self {
        DerivationInputs {
            srcs: srcs.clone(),
            drvs: BTreeMap::new(),
        }
    }
}

/// Borrowed counterpart of [`OutputInputs`] for zero-clone serialization.
#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct OutputInputsRef<'a> {
    outputs: BTreeSet<&'a OutputName>,
    dynamic_outputs: BTreeMap<&'a OutputName, OutputInputsRef<'a>>,
}

/// Borrowed counterpart of [`DerivationInputs`] for zero-clone serialization.
#[derive(Default, Serialize)]
pub(super) struct DerivationInputsRef<'a> {
    srcs: BTreeSet<&'a StorePath>,
    drvs: BTreeMap<&'a StorePath, OutputInputsRef<'a>>,
}

trait OutputNode<'a>: Default {
    fn insert_output(&mut self, output: &'a OutputName);
    fn dynamic_slot(&mut self, output: &'a OutputName) -> &mut Self;
}

impl<'a> OutputNode<'a> for OutputInputs {
    fn insert_output(&mut self, output: &'a OutputName) {
        self.outputs.insert(output.clone());
    }
    fn dynamic_slot(&mut self, output: &'a OutputName) -> &mut Self {
        self.dynamic_outputs.entry(output.clone()).or_default()
    }
}

impl<'a> OutputNode<'a> for OutputInputsRef<'a> {
    fn insert_output(&mut self, output: &'a OutputName) {
        self.outputs.insert(output);
    }
    fn dynamic_slot(&mut self, output: &'a OutputName) -> &mut Self {
        self.dynamic_outputs.entry(output).or_default()
    }
}

fn populate_inputs<'a, K, N>(
    paths: &'a BTreeSet<SingleDerivedPath>,
    srcs: &mut BTreeSet<K>,
    drvs: &mut BTreeMap<K, N>,
    mut to_key: impl FnMut(&'a StorePath) -> K,
) where
    K: Ord,
    N: OutputNode<'a>,
{
    fn ensure_slot<'a, 'b, K: Ord, N: OutputNode<'a>>(
        drvs: &'b mut BTreeMap<K, N>,
        k: &'a SingleDerivedPath,
        to_key: &mut impl FnMut(&'a StorePath) -> K,
    ) -> &'b mut N {
        match k {
            SingleDerivedPath::Opaque(store_path) => drvs.entry(to_key(store_path)).or_default(),
            SingleDerivedPath::Built { drv_path, output } => {
                ensure_slot(drvs, drv_path.as_ref(), to_key).dynamic_slot(output)
            }
        }
    }

    for path in paths {
        match path {
            SingleDerivedPath::Opaque(store_path) => {
                srcs.insert(to_key(store_path));
            }
            SingleDerivedPath::Built { drv_path, output } => {
                ensure_slot(drvs, drv_path.as_ref(), &mut to_key).insert_output(output);
            }
        }
    }
}

impl From<&BTreeSet<SingleDerivedPath>> for DerivationInputs {
    fn from(paths: &BTreeSet<SingleDerivedPath>) -> Self {
        let mut result = Self::default();
        populate_inputs(paths, &mut result.srcs, &mut result.drvs, Clone::clone);
        result
    }
}

impl<'a> From<&'a BTreeSet<SingleDerivedPath>> for DerivationInputsRef<'a> {
    fn from(paths: &'a BTreeSet<SingleDerivedPath>) -> Self {
        let mut result = Self::default();
        populate_inputs(paths, &mut result.srcs, &mut result.drvs, |p| p);
        result
    }
}
