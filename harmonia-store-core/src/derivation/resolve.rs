use std::collections::BTreeMap;

use crate::ByteString;
use crate::derived_path::{OutputName, SingleDerivedPath};
use crate::placeholder::Placeholder;
use crate::store_path::{StoreDir, StorePath, StorePathSet};

use super::{BasicDerivation, Derivation, DerivationOutput};

impl Derivation {
    /// Resolve input derivation references into concrete store paths.
    ///
    /// The derivation must be content-addressed. Input-addressed derivations
    /// don't need resolution since their output paths are already determined.
    ///
    /// Returns a [`BasicDerivation`] where all [`Built`](SingleDerivedPath::Built) inputs
    /// have been flattened into store path inputs, with CA placeholder strings rewritten
    /// to their actual paths.
    ///
    /// `query` resolves a `(SingleDerivedPath, output_name)` pair to the actual
    /// output store path. Returns [`None`] if the output hasn't been built yet.
    ///
    /// Returns [`None`] if any required input output path can't be resolved.
    pub fn try_resolve(
        &self,
        store_dir: &StoreDir,
        query: &mut impl FnMut(&SingleDerivedPath, &OutputName) -> Option<StorePath>,
    ) -> Option<BasicDerivation> {
        assert!(
            self.outputs.values().all(|o| matches!(
                o,
                DerivationOutput::CAFixed(_)
                    | DerivationOutput::CAFloating(_)
                    | DerivationOutput::Impure(_)
            )),
            "try_resolve requires a content-addressed derivation"
        );

        let mut input_srcs = StorePathSet::new();
        let mut input_rewrites = BTreeMap::<ByteString, ByteString>::new();

        for input in &self.inputs {
            match input {
                SingleDerivedPath::Opaque(path) => {
                    input_srcs.insert(path.clone());
                }
                SingleDerivedPath::Built { drv_path, output } => {
                    let actual_path = query(drv_path, output)?;

                    let placeholder = Placeholder::output(&drv_path.as_ref().into(), output);
                    let ph_path = placeholder.render();
                    let actual = actual_path.to_absolute_path(store_dir);
                    input_rewrites.insert(
                        ByteString::copy_from_slice(ph_path.as_os_str().as_encoded_bytes()),
                        ByteString::copy_from_slice(actual.as_os_str().as_encoded_bytes()),
                    );

                    input_srcs.insert(actual_path);
                }
            }
        }

        let mut resolved = BasicDerivation {
            name: self.name.clone(),
            outputs: self.outputs.clone(),
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
