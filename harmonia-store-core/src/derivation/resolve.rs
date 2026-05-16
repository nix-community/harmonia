use std::collections::BTreeMap;

use crate::ByteString;
use crate::derived_path::{OutputName, SingleDerivedPath};
use crate::placeholder::Placeholder;
use harmonia_store_path::{StoreDir, StorePath, StorePathSet};

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
    ///
    /// Any `InputAddressed` outputs to [`Deferred`](DerivationOutput::Deferred) since their
    /// original hashes are invalidated by the resolution.
    pub fn try_resolve(
        &self,
        store_dir: &StoreDir,
        query: &mut impl FnMut(&[(&SingleDerivedPath, &OutputName)]) -> Vec<Option<StorePath>>,
    ) -> Option<BasicDerivation> {
        let mut resolved = self.try_resolve_force(store_dir, query)?;

        // InputAddressed becomes Deferred when the inputs had Built
        // entries, since the original output hashes are no longer valid.
        let has_built_inputs = self
            .inputs
            .iter()
            .any(|i| matches!(i, SingleDerivedPath::Built { .. }));
        if has_built_inputs {
            for output in resolved.outputs.values_mut() {
                if matches!(output, DerivationOutput::InputAddressed(_)) {
                    *output = DerivationOutput::Deferred;
                }
            }
        }

        Some(resolved)
    }

    /// Resolve input derivation references into concrete store paths, keeping the original output
    /// definitions unchanged.
    ///
    /// Unlike [`try_resolve`](Self::try_resolve), this does *not* convert `InputAddressed` outputs
    /// to `Deferred`. The resulting derivation is not a valid resolved derivation (the output
    /// hashes don't match the resolved inputs). However, this invalid derivation is still useful
    /// for e.g. remote building where the builder should produce outputs at the
    /// originally-scheduled paths and need to know anything about how the inputs of the derivation
    /// were produced.
    pub fn try_resolve_force(
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde::Deserialize;

    use super::*;
    use crate::derivation::BasicDerivation;
    use harmonia_utils_test::json_upstream::{libstore_test_data_path, read_upstream_json};

    /// Just for testing purposes
    #[derive(Deserialize, PartialEq, Eq, PartialOrd, Ord)]
    struct BuildTraceKey {
        #[serde(rename = "drvPath")]
        drv_path: SingleDerivedPath,
        output: OutputName,
    }

    /// Just for testing purposes
    type BuildTrace = BTreeMap<BuildTraceKey, StorePath>;

    fn build_trace_query(
        trace: &BuildTrace,
    ) -> impl FnMut(&[(&SingleDerivedPath, &OutputName)]) -> Vec<Option<StorePath>> + '_ {
        |inputs: &[(&SingleDerivedPath, &OutputName)]| {
            inputs
                .iter()
                .map(|(drv_path, output)| {
                    trace
                        .get(&BuildTraceKey {
                            drv_path: (*drv_path).clone(),
                            output: (*output).clone(),
                        })
                        .cloned()
                })
                .collect()
        }
    }

    /// Run a `try_resolve` test case, mirroring upstream Nix's `resolveExpect`.
    ///
    /// Reads `{stem}-before.json`, `{stem}-buildTrace.json`, and
    /// `{stem}-after.json` from the upstream test data, then asserts
    /// that `try_resolve` produces the expected result.
    fn resolve_expect(stem: &str) {
        let store_dir = StoreDir::new("/nix/store").unwrap();

        let path = |suffix: &str| {
            libstore_test_data_path(&format!("derivation/try-resolve/{stem}-{suffix}.json"))
        };

        let drv: Derivation = read_upstream_json(&path("before"));

        let trace: BuildTrace =
            read_upstream_json::<Vec<(BuildTraceKey, StorePath)>>(&path("buildTrace"))
                .into_iter()
                .collect();

        let resolved = drv.try_resolve(&store_dir, &mut build_trace_query(&trace));

        let expected: BasicDerivation = read_upstream_json(&path("after"));
        assert_eq!(resolved, Some(expected));
    }

    #[test]
    fn no_inputs() {
        resolve_expect("no-inputs");
    }

    #[test]
    fn with_inputs() {
        resolve_expect("with-inputs");
    }

    #[test]
    fn resolution_failure() {
        let store_dir = StoreDir::new("/nix/store").unwrap();
        let drv: Derivation = read_upstream_json(&libstore_test_data_path(
            "derivation/try-resolve/resolution-failure-before.json",
        ));
        let resolved = drv.try_resolve(&store_dir, &mut build_trace_query(&BuildTrace::new()));
        assert!(resolved.is_none());
    }
}
