use std::collections::BTreeMap;

use crate::ByteString;
use crate::derived_path::{OutputName, SingleDerivedPath};
use crate::placeholder::Placeholder;
use crate::store_path::{StoreDir, StorePath, StorePathSet};

use super::{BasicDerivation, Derivation, DerivationOutput};

impl Derivation {
    /// Resolve input derivation references into concrete store paths.
    ///
    /// While doing that, rewrites CA placeholder strings in
    /// builder/args/env to their actual output paths.
    ///
    /// For input-addressed outputs since the original input addresses
    /// will be invalidated, convert them to
    /// [`Deferred`](DerivationOutput::Deferred). Something else can
    /// recompute and fill in those input addresses.
    ///
    /// Returns a [`BasicDerivation`] where all [`Built`](SingleDerivedPath::Built) inputs
    /// have been flattened into store path inputs.
    ///
    /// `query` resolves a batch of `(&SingleDerivedPath, &OutputName)` pairs to their actual
    /// output store paths, returning a `Vec<Option<StorePath>>` of the same length. [`None`]
    /// indicates an output that hasn't been built yet.
    ///
    /// Returns [`None`] if any required input output path can't be resolved.
    pub fn try_resolve(
        &self,
        store_dir: &StoreDir,
        query: &mut impl FnMut(&[(&SingleDerivedPath, &OutputName)]) -> Vec<Option<StorePath>>,
    ) -> Option<BasicDerivation> {
        let mut input_srcs = StorePathSet::new();
        let mut input_rewrites = BTreeMap::<ByteString, ByteString>::new();

        let mut built_inputs: Vec<(&SingleDerivedPath, &OutputName)> = Vec::new();

        for input in &self.inputs {
            match input {
                SingleDerivedPath::Opaque(path) => {
                    input_srcs.insert(path.clone());
                }
                SingleDerivedPath::Built { drv_path, output } => {
                    built_inputs.push((drv_path, output));
                }
            }
        }

        let non_trivial_resolution = !built_inputs.is_empty();

        let resolved_paths = query(&built_inputs);
        assert_eq!(resolved_paths.len(), built_inputs.len());

        for ((drv_path, output), actual_path) in built_inputs.into_iter().zip(resolved_paths) {
            let actual_path = actual_path?;

            let placeholder = Placeholder::output(&drv_path.into(), output);
            let ph_path = placeholder.render();
            let actual = actual_path.to_absolute_path(store_dir);
            input_rewrites.insert(
                ByteString::copy_from_slice(ph_path.as_os_str().as_encoded_bytes()),
                ByteString::copy_from_slice(actual.as_os_str().as_encoded_bytes()),
            );

            input_srcs.insert(actual_path);
        }

        // InputAddressed becomes Deferred when the inputs had CA rewrites, since the
        // original paths are no longer valid.
        let outputs = self
            .outputs
            .iter()
            .map(|(name, output)| {
                let output = match output {
                    DerivationOutput::InputAddressed(_) if non_trivial_resolution => {
                        DerivationOutput::Deferred
                    }
                    other => other.clone(),
                };
                (name.clone(), output)
            })
            .collect();

        let mut resolved = BasicDerivation {
            name: self.name.clone(),
            outputs,
            inputs: input_srcs,
            platform: self.platform.clone(),
            builder: self.builder.clone(),
            args: self.args.clone(),
            env: self.env.clone(),
            structured_attrs: self.structured_attrs.clone(),
        };

        resolved.apply_rewrites(&input_rewrites);

        Some(resolved)
    }
}
