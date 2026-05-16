//! For derivations that input-address their outputs

use harmonia_store_core::ByteString;
use harmonia_store_core::derivation::{
    BasicDerivation, DerivationOutput, DerivationT, OutputPathName,
};
use harmonia_store_core::derived_path::OutputName;
use harmonia_store_core::store_path::{
    StoreDir, StorePath, StorePathName, StorePathNameError, StorePathSet,
};

use crate::error::ParseError;
use crate::print_derivation_aterm;
use crate::raw_output::{AtermOutput, BorrowedRawOutput, RawOutput};

/// A placeholder output type for derivations whose output paths have
/// not yet been computed. Prints as all-empty fields in ATerm format,
/// the same as [`DerivationOutput::Deferred`].
///
/// Use [`fill_outputs`] to compute paths and convert to
/// `DerivationT<StorePathSet, StorePath>`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct UnfilledOutput;

impl From<UnfilledOutput> for DerivationOutput {
    fn from(UnfilledOutput {}: UnfilledOutput) -> Self {
        Self::Deferred
    }
}

impl AtermOutput for UnfilledOutput {
    fn to_raw(
        &self,
        _store_dir: &StoreDir,
        _drv_name: &StorePathName,
        _output_name: &OutputName,
    ) -> RawOutput {
        RawOutput {
            path: Vec::new(),
            hash_algo: Vec::new(),
            hash: Vec::new(),
        }
    }

    fn from_raw(
        _raw: BorrowedRawOutput,
        _store_dir: &StoreDir,
        _drv_name: &StorePathName,
        _output_name: &OutputName,
    ) -> Result<Self, ParseError> {
        Ok(UnfilledOutput)
    }
}

/// Hash a resolved derivation with unfilled outputs by printing it to
/// ATerm and taking the SHA-256 digest.
///
/// Because the input is a resolved `BasicDerivation` — all inputs are
/// already concrete store paths, not derivation references — the hash
/// is simply `sha256(aterm)`. This avoids the full recursive
/// `hashDerivationModulo` algorithm that a [`Derivation`](harmonia_store_core::derivation::Derivation)
/// with unresolved input derivation references would require.
pub fn hash_derivation(
    store_dir: &StoreDir,
    drv: &DerivationT<StorePathSet, UnfilledOutput>,
) -> harmonia_utils_hash::Sha256 {
    // Convert UnfilledOutput → DerivationOutput::Deferred so the
    // printer can produce the ATerm with all-empty output fields.
    let as_basic: BasicDerivation = drv.clone().map_outputs(DerivationOutput::from);
    let aterm = print_derivation_aterm(store_dir, &as_basic);
    harmonia_utils_hash::Sha256::digest(&aterm)
}

/// Compute the output store path for an input-addressed derivation
/// output, given the derivation hash and output name.
///
/// The derivation hash can be obtained from [`hash_derivation`].
pub fn make_output_path(
    store_dir: &StoreDir,
    drv_name: &StorePathName,
    drv_hash: &harmonia_utils_hash::Sha256,
    output_name: &OutputName,
) -> Result<StorePath, StorePathNameError> {
    let output_path_name = OutputPathName {
        drv_name,
        output_name,
    }
    .to_string()
    .parse::<StorePathName>()?;

    let fingerprint = format!(
        "output:{}:sha256:{:x}:{}:{}",
        output_name, drv_hash, store_dir, output_path_name,
    );
    let path_hash = harmonia_utils_hash::Sha256::digest(fingerprint);
    Ok(StorePath::from_hash(&path_hash, output_path_name))
}

/// Compute input-addressed output paths for a resolved derivation,
/// returning a new derivation with [`StorePath`] outputs.
///
/// The output paths are computed from the derivation's ATerm hash (via
/// [`hash_derivation`] and [`make_output_path`]).
///
/// The `env` map is also updated so that output name variables (e.g.
/// `$out`) contain the computed absolute store path.
pub fn fill_outputs(
    store_dir: &StoreDir,
    drv: DerivationT<StorePathSet, UnfilledOutput>,
) -> Result<DerivationT<StorePathSet, StorePath>, StorePathNameError> {
    let drv_hash = hash_derivation(store_dir, &drv);

    let mut outputs = std::collections::BTreeMap::new();
    let mut env = drv.env;

    for (output_name, UnfilledOutput) in drv.outputs {
        let path = make_output_path(store_dir, &drv.name, &drv_hash, &output_name)?;

        let abs_path = path
            .to_absolute_path(store_dir)
            .to_string_lossy()
            .into_owned();
        env.insert(
            ByteString::from(output_name.as_ref().to_owned()),
            ByteString::from(abs_path),
        );

        outputs.insert(output_name, path);
    }

    Ok(DerivationT {
        name: drv.name,
        outputs,
        inputs: drv.inputs,
        platform: drv.platform,
        builder: drv.builder,
        args: drv.args,
        env,
        structured_attrs: drv.structured_attrs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use harmonia_store_core::derivation::DerivationOutputs;
    use harmonia_store_core::derivation::derivation_output_arbitrary::arb_output_name_for_drv;
    use proptest::prelude::*;

    /// Generate a `DerivationT<StorePathSet, UnfilledOutput>` whose output
    /// names are guaranteed to form valid `OutputPathName`s with the drv name.
    fn arb_unfilled_drv() -> impl Strategy<Value = DerivationT<StorePathSet, UnfilledOutput>> {
        any::<BasicDerivation>().prop_flat_map(|drv| {
            let name = drv.name.clone();
            let num_outputs = drv.outputs.len().max(1);
            proptest::collection::btree_map(
                arb_output_name_for_drv(&name),
                Just(UnfilledOutput),
                1..=num_outputs,
            )
            .prop_map(move |outputs| DerivationT {
                name: drv.name.clone(),
                outputs,
                inputs: drv.inputs.clone(),
                platform: drv.platform.clone(),
                builder: drv.builder.clone(),
                args: drv.args.clone(),
                env: drv.env.clone(),
                structured_attrs: drv.structured_attrs.clone(),
            })
        })
    }

    proptest! {
        #[test]
        fn unfilled_output_roundtrips(
            drv_name: StorePathName,
            output_name: OutputName,
        ) {
            let store_dir = StoreDir::default();
            let raw = UnfilledOutput.to_raw(&store_dir, &drv_name, &output_name);
            prop_assert!(raw.path.is_empty());
            prop_assert!(raw.hash_algo.is_empty());
            prop_assert!(raw.hash.is_empty());
            let roundtripped = UnfilledOutput::from_raw(
                raw.borrow(), &store_dir, &drv_name, &output_name,
            ).unwrap();
            prop_assert_eq!(UnfilledOutput, roundtripped);
        }

        /// `hash_derivation` should be deterministic — same derivation
        /// always produces the same hash.
        #[test]
        fn hash_derivation_deterministic(drv in arb_unfilled_drv()) {
            let store_dir = StoreDir::default();
            let h1 = hash_derivation(&store_dir, &drv);
            let h2 = hash_derivation(&store_dir, &drv);
            prop_assert_eq!(h1, h2);
        }

        /// `fill_outputs` should produce output paths that are consistent
        /// with calling `make_output_path` individually.
        #[test]
        fn fill_outputs_consistent_with_make_output_path(drv in arb_unfilled_drv()) {
            let store_dir = StoreDir::default();
            let drv_hash = hash_derivation(&store_dir, &drv);
            let drv_name = drv.name.clone();
            let output_names: Vec<_> = drv.outputs.keys().cloned().collect();

            let filled = fill_outputs(&store_dir, drv).unwrap();

            for output_name in &output_names {
                let expected = make_output_path(
                    &store_dir, &drv_name, &drv_hash, output_name,
                ).unwrap();
                let actual = filled.outputs.get(output_name).unwrap();
                prop_assert_eq!(&expected, actual);
            }
        }

        /// `fill_outputs` should update env vars to match output paths.
        #[test]
        fn fill_outputs_updates_env(drv in arb_unfilled_drv()) {
            let store_dir = StoreDir::default();
            let output_names: Vec<_> = drv.outputs.keys().cloned().collect();

            let filled = fill_outputs(&store_dir, drv).unwrap();

            for output_name in &output_names {
                let path = filled.outputs.get(output_name).unwrap();
                let expected_env = path
                    .to_absolute_path(&store_dir)
                    .to_string_lossy()
                    .into_owned();
                let env_key = ByteString::from(output_name.as_ref().to_owned());
                let actual_env = filled.env.get(&env_key).expect("env var should be set");
                prop_assert_eq!(expected_env.as_bytes(), actual_env.as_ref());
            }
        }

        /// Different derivations should (almost certainly) produce
        /// different hashes.
        #[test]
        fn hash_derivation_varies(
            drv1 in arb_unfilled_drv(),
            drv2 in arb_unfilled_drv(),
        ) {
            // Only assert if the derivations are actually different
            if drv1.name != drv2.name
                || drv1.inputs != drv2.inputs
                || drv1.platform != drv2.platform
                || drv1.builder != drv2.builder
                || drv1.args != drv2.args
            {
                let store_dir = StoreDir::default();
                let h1 = hash_derivation(&store_dir, &drv1);
                let h2 = hash_derivation(&store_dir, &drv2);
                prop_assert_ne!(h1, h2);
            }
        }
    }

    /// Outputs should be preserved (same count, same names) through
    /// `fill_outputs`.
    #[test]
    fn fill_outputs_preserves_output_names() {
        let store_dir = StoreDir::default();
        let mut outputs = DerivationOutputs::new();
        outputs.insert("out".parse().unwrap(), UnfilledOutput);
        outputs.insert("dev".parse().unwrap(), UnfilledOutput);
        outputs.insert("lib".parse().unwrap(), UnfilledOutput);

        let drv = DerivationT {
            name: "test".parse().unwrap(),
            outputs,
            inputs: StorePathSet::new(),
            platform: ByteString::from("x86_64-linux"),
            builder: ByteString::from("/bin/sh"),
            args: vec![],
            env: std::collections::BTreeMap::new(),
            structured_attrs: None,
        };

        let filled = fill_outputs(&store_dir, drv).unwrap();
        assert_eq!(filled.outputs.len(), 3);
        assert!(filled.outputs.contains_key(&"out".parse().unwrap()));
        assert!(filled.outputs.contains_key(&"dev".parse().unwrap()));
        assert!(filled.outputs.contains_key(&"lib".parse().unwrap()));

        // Each output should have a unique path
        let paths: std::collections::HashSet<_> = filled.outputs.values().collect();
        assert_eq!(paths.len(), 3);
    }
}
