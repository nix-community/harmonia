//! Derivation hashes "modulo" fixed-output derivations, for computing
//! input-addressed output paths of *unresolved* derivations.
//!
//! We don't want changes to fixed-output derivations to propagate
//! upwards through the dependency graph, changing output paths
//! everywhere: if the url in a `fetchurl` call changes but the content
//! hash does not, nothing that depends on it needs rebuilding. So
//! before hashing, each input derivation is replaced by its own "hash
//! modulo": a fixed-output derivation contributes a hash derived only
//! from its content address, while a regular derivation contributes
//! the hash of itself after its inputs have likewise been replaced.
//!
//! This is the recursive counterpart to
//! [`input_address::hash_derivation`](crate::input_address::hash_derivation),
//! which handles only *resolved* derivations (no input derivations).
//! It mirrors Nix's `hashDerivationModulo`, computing the hash via an
//! intermediate derivation whose `inputDrvs` are keyed by hash (rendered
//! bare base16) rather than by store path.
//!
//! There is no `Store` in this crate, so looking up an input derivation's
//! hash modulo is delegated to a caller-supplied closure. The caller is
//! also responsible for memoization; a natural implementation computes
//! hashes bottom-up over the derivation closure, caching by `.drv` path.

use std::collections::{BTreeMap, BTreeSet};

use bytes::Bytes;

use harmonia_store_derivation::derivation::{
    Derivation, DerivationInputs, DerivationOutput, DerivationT, OutputInputs,
};
use harmonia_store_derivation::derived_path::{OutputName, SingleDerivedPath};
#[cfg(test)]
use harmonia_store_path::StorePathSet;
use harmonia_store_path::{StoreDir, StorePath, StorePathNameError};
use harmonia_utils_hash::Sha256;

use crate::input_address::{UnfilledOutput, fill_outputs_from_hash};
use crate::print_derivation_aterm;

/// The hash of a derivation "modulo" fixed-output derivations, used as
/// its identity when it appears as an input to other derivations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HashModulo {
    /// Single hash for the derivation.
    ///
    /// This is for an input-addressed derivation that doesn't
    /// transitively depend on any floating-CA derivations.
    DrvHash(Sha256),
    /// Known CA output hashes, for fixed-output derivations whose
    /// output hashes are always known since they are fixed up-front.
    ///
    /// Each hash is not the corresponding output's content hash, but a
    /// hash *of* that hash along with other constant data: a pure
    /// function of the output's contents that doesn't leak the
    /// provenance of the fixed output (reducing pointless cache misses,
    /// as the build itself won't know it either).
    CaOutputHashes(BTreeMap<OutputName, Sha256>),
    /// This derivation doesn't yet have known output hashes, either
    /// because it is itself floating-CA or impure, or because it
    /// (transitively) depends on such a derivation (or on a dynamic
    /// derivation output, which must be resolved first).
    DeferredDrv,
}

/// The inputs of the intermediate derivation used to compute the hash
/// modulo: input derivations are identified by their hash modulo rather
/// than by store path.
///
/// `Sha256`'s `Ord` compares digest bytes left-to-right, which matches
/// base16-lexicographic order (hex encoding is monotonic per byte), so
/// the `BTreeMap` gives the correct ATerm key ordering directly.
type HashModuloInputs = DerivationInputs<Sha256>;

/// Error from substituting input derivations with their hash moduli —
/// the failure modes shared by every function in this module.
#[derive(Debug, thiserror::Error)]
pub enum InputModuloError<E> {
    /// The caller-supplied input-derivation lookup failed.
    #[error("looking up input derivation hash modulo: {0}")]
    Lookup(#[source] E),
    /// A fixed-output input derivation's hash modulo has no entry for a
    /// requested output.
    #[error("no hash for output '{output}' of an input derivation")]
    MissingOutputHash { output: OutputName },
}

/// Replace each input derivation with its hash modulo, producing the
/// input map of the intermediate derivation.
///
/// Returns `Ok(None)` if any input is deferred or dynamic (its output
/// paths are not yet known, so no hash modulo can be computed yet).
fn modulo_drvs<E>(
    inputs: &DerivationInputs,
    mut lookup: impl FnMut(&StorePath) -> Result<HashModulo, E>,
) -> Result<Option<BTreeMap<Sha256, OutputInputs>>, InputModuloError<E>> {
    let mut drvs: BTreeMap<Sha256, OutputInputs> = BTreeMap::new();
    for (drv_path, oi) in &inputs.drvs {
        // Need to build and resolve dynamic derivations first.
        if !oi.dynamic_outputs.is_empty() {
            return Ok(None);
        }
        // Two input derivation paths can share a hash modulo (that is
        // the point of the modulo — e.g. they differ only in
        // fixed-output provenance), so MERGE output sets on a hash-key
        // collision. (Nix `insert_or_assign`s here, silently
        // dropping the earlier entry's outputs from the intermediate
        // ATerm — a bug we deliberately do not reproduce.)
        /// Merge one intermediate `inputDrvs` entry (see the collision
        /// note above).
        fn add(
            drvs: &mut BTreeMap<Sha256, OutputInputs>,
            h: Sha256,
            names: impl IntoIterator<Item = OutputName>,
        ) {
            drvs.entry(h).or_default().outputs.extend(names);
        }
        match lookup(drv_path).map_err(InputModuloError::Lookup)? {
            HashModulo::DeferredDrv => return Ok(None),
            // Regular non-CA derivation: replace the derivation path
            // with its hash, keeping the requested output names.
            HashModulo::DrvHash(h) => add(&mut drvs, h, oi.outputs.iter().cloned()),
            // Fixed-output derivation: pretend each output hash is a
            // derivation hash producing a single "out" output.
            HashModulo::CaOutputHashes(output_hashes) => {
                for output in &oi.outputs {
                    let h = output_hashes.get(output).ok_or_else(|| {
                        InputModuloError::MissingOutputHash {
                            output: output.clone(),
                        }
                    })?;
                    add(&mut drvs, *h, [OutputName::default()]);
                }
            }
        }
    }
    Ok(Some(drvs))
}

/// Replace each input derivation with its hash modulo, producing the
/// intermediate derivation the hash is taken over. Polymorphic over the
/// output type: `hash_input_modulo` preserves outputs, `hash_modulo`
/// masks them first.
///
/// Returns `Ok(None)` if any input is deferred or dynamic.
fn derivation_modulo<E, O>(
    drv: DerivationT<BTreeSet<SingleDerivedPath>, O>,
    lookup: impl FnMut(&StorePath) -> Result<HashModulo, E>,
) -> Result<Option<DerivationT<HashModuloInputs, O>>, InputModuloError<E>> {
    let inputs = DerivationInputs::from(&drv.inputs);
    let Some(drvs) = modulo_drvs(&inputs, lookup)? else {
        return Ok(None);
    };
    Ok(Some(drv.map_inputs(|_| HashModuloInputs {
        srcs: inputs.srcs,
        drvs,
    })))
}

/// Substitute the inputs with their hash moduli and hash the printed
/// intermediate derivation. `None` if any input is deferred or dynamic.
fn hash_intermediate<E, O: crate::raw_output::AtermOutput>(
    store_dir: &StoreDir,
    drv: DerivationT<BTreeSet<SingleDerivedPath>, O>,
    lookup: impl FnMut(&StorePath) -> Result<HashModulo, E>,
) -> Result<Option<Sha256>, InputModuloError<E>> {
    let Some(intermediate) = derivation_modulo(drv, lookup)? else {
        return Ok(None);
    };
    Ok(Some(Sha256::digest(print_derivation_aterm(
        store_dir,
        &intermediate,
    ))))
}

/// Is this a fixed-output derivation (all outputs `CAFixed`)?
fn is_fixed(drv: &Derivation) -> bool {
    !drv.outputs.is_empty()
        && drv
            .outputs
            .values()
            .all(|o| matches!(o, DerivationOutput::CAFixed(_)))
}

/// Error from [`hash_input_modulo`].
#[derive(Debug, thiserror::Error)]
pub enum HashInputModuloError<E> {
    #[error(transparent)]
    InputModulo(#[from] InputModuloError<E>),
    /// A fixed output's path name could not be formed from the
    /// derivation and output names.
    #[error(transparent)]
    StorePathName(#[from] StorePathNameError),
}

/// Compute the hash of `drv` with outputs preserved: its identity as an
/// input to other derivations.
///
/// `lookup` must return the hash modulo of the derivation at the given
/// store path (each of `drv`'s input derivations will be looked up).
/// The caller is responsible for memoizing it.
///
/// Returns:
/// - [`HashModulo::CaOutputHashes`] for fixed-output derivations;
/// - [`HashModulo::DeferredDrv`] if `drv` has any non-input-addressed
///   output, or any input is itself deferred or dynamic;
/// - [`HashModulo::DrvHash`] for regular input-addressed derivations.
pub fn hash_input_modulo<E>(
    store_dir: &StoreDir,
    drv: &Derivation,
    lookup: impl FnMut(&StorePath) -> Result<HashModulo, E>,
) -> Result<HashModulo, HashInputModuloError<E>> {
    // Return a fixed hash for fixed-output derivations.
    if is_fixed(drv) {
        let mut output_hashes = BTreeMap::new();
        for (output_name, output) in &drv.outputs {
            let DerivationOutput::CAFixed(ca) = output else {
                unreachable!("is_fixed checked all outputs are CAFixed");
            };
            let path = output
                .path(store_dir, &drv.name, output_name)?
                .expect("CAFixed always has a path");
            // `{:#x}` renders the bare base16 digest (the non-alternate
            // form prefixes the algorithm, which `method_algorithm()`
            // already provides here).
            let fingerprint = format!(
                "fixed:out:{}:{:#x}:{}",
                ca.method_algorithm(),
                ca.hash().to_owned(),
                path.to_absolute_path(store_dir).display(),
            );
            output_hashes.insert(output_name.clone(), Sha256::digest(fingerprint));
        }
        return Ok(HashModulo::CaOutputHashes(output_hashes));
    }

    // If any output is not InputAddressed, this derivation has no hash
    // modulo yet.
    if drv
        .outputs
        .values()
        .any(|o| !matches!(o, DerivationOutput::InputAddressed(_)))
    {
        return Ok(HashModulo::DeferredDrv);
    }

    Ok(match hash_intermediate(store_dir, drv.clone(), lookup)? {
        Some(h) => HashModulo::DrvHash(h),
        None => HashModulo::DeferredDrv,
    })
}

/// Replace the outputs with unfilled placeholders and blank the env
/// vars named after them, so the hash does not depend on the
/// derivation's own output paths.
///
/// Purely structural — whether masking is *meaningful* for the given
/// output type is the caller's concern (e.g. [`hash_modulo`] first
/// checks the derivation is input-addressed).
pub(crate) fn mask_outputs_and_env<I, O>(drv: DerivationT<I, O>) -> DerivationT<I, UnfilledOutput> {
    let mut masked = drv.map_outputs(|_| UnfilledOutput);
    for output_name in masked.outputs.keys() {
        let key = Bytes::copy_from_slice(output_name.as_ref().as_bytes());
        if let Some(v) = masked.env.get_mut(&key) {
            *v = Bytes::new();
        }
    }
    masked
}

/// Error from [`hash_modulo`].
#[derive(Debug, thiserror::Error)]
pub enum HashModuloError<E> {
    #[error(transparent)]
    InputModulo(#[from] InputModuloError<E>),
    /// The derivation is not input-addressed (it has CA-fixed,
    /// CA-floating, or impure outputs), so it has no masked hash
    /// modulo: such outputs have no statically computable
    /// input-addressed paths.
    #[error("output '{output}' is not input-addressed (or deferred)")]
    NotInputAddressed { output: OutputName },
}

/// Compute the hash of `drv` with outputs masked, for computing its
/// *own* output paths (rather than its identity as an input to other
/// derivations). Only valid for input-addressed (possibly deferred)
/// derivations.
///
/// Returns `Ok(None)` if the hash cannot be computed yet because some
/// input's output paths are not yet known.
pub fn hash_modulo<E>(
    store_dir: &StoreDir,
    drv: &Derivation,
    lookup: impl FnMut(&StorePath) -> Result<HashModulo, E>,
) -> Result<Option<Sha256>, HashModuloError<E>> {
    // Masking is only meaningful for input-addressed (possibly
    // deferred) derivations; CA outputs have no input-addressed paths.
    for (output_name, output) in &drv.outputs {
        match output {
            DerivationOutput::InputAddressed(_) => {}
            // Possibly pessimistically deferred --- we will fill in the
            // output paths.
            DerivationOutput::Deferred => {}
            _ => {
                return Err(HashModuloError::NotInputAddressed {
                    output: output_name.clone(),
                });
            }
        }
    }
    let masked = mask_outputs_and_env(drv.clone());
    Ok(hash_intermediate(store_dir, masked, lookup)?)
}

/// Error from [`fill_deferred_outputs`].
#[derive(Debug, thiserror::Error)]
pub enum FillOutputsError<E> {
    #[error(transparent)]
    InputModulo(#[from] InputModuloError<E>),
    /// An output path name could not be formed from the derivation and
    /// output names.
    #[error(transparent)]
    StorePathName(#[from] StorePathNameError),
}

/// Compute input-addressed output paths for a (possibly unresolved)
/// derivation with deferred outputs, returning a new derivation with
/// every output filled in as a [`StorePath`] and the corresponding env
/// vars (e.g. `$out`) set to the absolute store paths.
///
/// This is the unresolved counterpart of
/// [`input_address::fill_outputs`](crate::input_address::fill_outputs).
///
/// Returns `Ok(None)` if the output paths cannot be computed *yet*:
/// some input's output paths are not known (the derivation stays
/// deferred until its inputs are resolved).
///
/// Taking [`UnfilledOutput`] outputs enforces "all outputs deferred" in
/// the type: the caller classifies the derivation (CA vs.
/// input-addressed) and converts with
/// `map_outputs(|_| UnfilledOutput)`, mirroring the resolved-path
/// [`input_address::fill_outputs`](crate::input_address::fill_outputs).
pub fn fill_deferred_outputs<E>(
    store_dir: &StoreDir,
    drv: DerivationT<BTreeSet<SingleDerivedPath>, UnfilledOutput>,
    lookup: impl FnMut(&StorePath) -> Result<HashModulo, E>,
) -> Result<Option<DerivationT<BTreeSet<SingleDerivedPath>, StorePath>>, FillOutputsError<E>> {
    // The output type already guarantees all outputs are deferred; mask
    // a copy for hashing (the real env keeps its values until the
    // computed paths overwrite them in fill_outputs_from_hash).
    let masked = mask_outputs_and_env(drv.clone());
    let Some(drv_hash) = hash_intermediate(store_dir, masked, lookup)? else {
        return Ok(None);
    };
    Ok(Some(fill_outputs_from_hash(store_dir, drv, &drv_hash)?))
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;

    use super::*;
    use harmonia_store_content_address::ContentAddress;

    fn store_dir() -> StoreDir {
        StoreDir::default()
    }

    /// A minimal input-addressed derivation with `Deferred` outputs and
    /// the given inputs.
    fn deferred_drv(name: &str, inputs: BTreeSet<SingleDerivedPath>) -> Derivation {
        let mut drv = Derivation::new(
            name.parse().unwrap(),
            Bytes::from("x86_64-linux"),
            Bytes::from("/bin/sh"),
        );
        drv.inputs = inputs;
        drv.outputs
            .insert(OutputName::default(), DerivationOutput::Deferred);
        drv.env.insert(Bytes::from("out"), Bytes::from("replaced"));
        drv
    }

    /// A fixed-output derivation with the given content address and an
    /// arbitrary impurity (env var) that must not leak downstream.
    fn fixed_drv(name: &str, ca: &str, env_noise: &str) -> Derivation {
        let mut drv = Derivation::new(
            name.parse().unwrap(),
            Bytes::from("x86_64-linux"),
            Bytes::from("/bin/sh"),
        );
        drv.outputs.insert(
            OutputName::default(),
            DerivationOutput::CAFixed(ca.parse::<ContentAddress>().unwrap()),
        );
        drv.env
            .insert(Bytes::from("url"), Bytes::from(env_noise.to_owned()));
        drv
    }

    fn no_lookup(_: &StorePath) -> Result<HashModulo, Infallible> {
        panic!("derivation has no input derivations; lookup must not be called")
    }

    const CA: &str =
        "fixed:sha256:248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1";

    /// Changing a fixed-output derivation without changing its content
    /// address must not change the hash modulo of anything downstream
    /// (e.g. a changed `fetchurl` url with the same content hash).
    #[test]
    fn fixed_output_provenance_hidden() {
        let sd = store_dir();
        let fixed_a = fixed_drv("dep-1.0", CA, "https://a.example/tarball");
        let fixed_b = fixed_drv("dep-1.0", CA, "https://b.example/tarball");
        let ha = hash_input_modulo(&sd, &fixed_a, no_lookup).unwrap();
        let hb = hash_input_modulo(&sd, &fixed_b, no_lookup).unwrap();
        assert!(matches!(ha, HashModulo::CaOutputHashes(_)));
        assert_eq!(ha, hb);

        // A downstream derivation depending on either gets the same
        // output paths.
        let drv_path: StorePath = "00000000000000000000000000000000-dep-1.0.drv"
            .parse()
            .unwrap();
        let inputs: BTreeSet<_> = [SingleDerivedPath::Built {
            drv_path: std::sync::Arc::new(SingleDerivedPath::Opaque(drv_path.clone())),
            output: OutputName::default(),
        }]
        .into_iter()
        .collect();
        let downstream = deferred_drv("app-1.0", inputs);

        let fill = |modulo: HashModulo| {
            fill_deferred_outputs(
                &sd,
                downstream.clone().map_outputs(|_| UnfilledOutput),
                |p: &StorePath| {
                    assert_eq!(*p, drv_path);
                    Ok::<_, Infallible>(modulo.clone())
                },
            )
            .unwrap()
            .expect("all inputs known; outputs must be computable")
        };
        assert_eq!(fill(ha), fill(hb));
    }

    /// For a derivation with no input derivations, `fill_deferred_outputs`
    /// must agree with the resolved-derivation path
    /// ([`crate::input_address::fill_outputs`]).
    #[test]
    fn resolved_matches_basic_fill() {
        let sd = store_dir();
        let src: StorePath = "11111111111111111111111111111111-src".parse().unwrap();
        let inputs: BTreeSet<_> = [SingleDerivedPath::Opaque(src.clone())]
            .into_iter()
            .collect();
        let drv = deferred_drv("app-1.0", inputs);

        let filled =
            fill_deferred_outputs(&sd, drv.clone().map_outputs(|_| UnfilledOutput), no_lookup)
                .unwrap()
                .expect("no input derivations; outputs must be computable");

        // Same derivation, expressed as a resolved BasicDerivation with
        // unfilled outputs, run through the pre-existing resolved path.
        let basic = drv
            .map_inputs(|inputs| {
                inputs
                    .into_iter()
                    .map(|p| match p {
                        SingleDerivedPath::Opaque(p) => p,
                        _ => unreachable!(),
                    })
                    .collect::<StorePathSet>()
            })
            .map_outputs(|_| UnfilledOutput);
        let basic_filled = crate::input_address::fill_outputs(&sd, basic).unwrap();

        assert_eq!(filled.outputs, basic_filled.outputs);
        assert_eq!(filled.env, basic_filled.env);
    }

    /// Floating-CA outputs: no hash modulo as an input, and an error
    /// when asking for input-addressed output paths.
    #[test]
    fn floating_ca_is_deferred() {
        let sd = store_dir();
        let mut drv = deferred_drv("app-1.0", BTreeSet::new());
        drv.outputs.insert(
            OutputName::default(),
            DerivationOutput::CAFloating("r:sha256".parse().unwrap()),
        );
        assert_eq!(
            hash_input_modulo(&sd, &drv, no_lookup).unwrap(),
            HashModulo::DeferredDrv
        );
        assert!(matches!(
            hash_modulo(&sd, &drv, no_lookup),
            Err(HashModuloError::NotInputAddressed { .. })
        ));
        // (fill_deferred_outputs cannot even be called here: its
        // UnfilledOutput parameter excludes CA outputs at compile time.)
    }

    /// An input whose own hash modulo is deferred defers this
    /// derivation's output-path computation too.
    #[test]
    fn deferred_input_defers() {
        let sd = store_dir();
        let drv_path: StorePath = "00000000000000000000000000000000-dep-1.0.drv"
            .parse()
            .unwrap();
        let inputs: BTreeSet<_> = [SingleDerivedPath::Built {
            drv_path: std::sync::Arc::new(SingleDerivedPath::Opaque(drv_path)),
            output: OutputName::default(),
        }]
        .into_iter()
        .collect();
        let drv = deferred_drv("app-1.0", inputs);
        let lookup = |_: &StorePath| Ok::<_, Infallible>(HashModulo::DeferredDrv);
        assert_eq!(
            hash_input_modulo(&sd, &drv, lookup).unwrap(),
            HashModulo::DeferredDrv
        );
        assert_eq!(hash_modulo(&sd, &drv, lookup).unwrap(), None);
        assert_eq!(
            fill_deferred_outputs(&sd, drv.map_outputs(|_| UnfilledOutput), lookup).unwrap(),
            None
        );
    }

    /// The identity hash (outputs preserved) and the masked own-path
    /// hash must differ: the former sees the filled output paths.
    #[test]
    fn identity_and_masked_hashes_differ() {
        let sd = store_dir();
        let drv = deferred_drv("app-1.0", BTreeSet::new());
        let filled_paths =
            fill_deferred_outputs(&sd, drv.map_outputs(|_| UnfilledOutput), no_lookup)
                .unwrap()
                .unwrap();
        let filled = filled_paths
            .clone()
            .map_outputs(DerivationOutput::InputAddressed);

        let identity = hash_input_modulo(&sd, &filled, no_lookup).unwrap();
        let HashModulo::DrvHash(identity) = identity else {
            panic!("input-addressed derivation must have a DrvHash");
        };
        let masked = hash_modulo(&sd, &filled, no_lookup).unwrap().unwrap();
        assert_ne!(identity, masked);

        // And re-filling from the masked hash is a fixpoint: the
        // computed paths match the ones already present.
        let refilled = fill_deferred_outputs(
            &sd,
            filled.clone().map_outputs(|_| UnfilledOutput),
            no_lookup,
        )
        .unwrap()
        .unwrap();
        assert_eq!(refilled, filled_paths);
    }
}
