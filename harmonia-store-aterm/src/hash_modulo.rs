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
use std::fmt::Write as _;

#[cfg(test)]
use bytes::Bytes;

#[cfg(test)]
use harmonia_store_derivation::derivation::DerivationInputs;
use harmonia_store_derivation::derivation::{
    Derivation, DerivationOutput, DerivationT, OutputInputs,
};
use harmonia_store_derivation::derived_path::{OutputName, SingleDerivedPath};
#[cfg(test)]
use harmonia_store_path::StorePathSet;
use harmonia_store_path::{StoreDir, StorePath, StorePathNameError};
use harmonia_utils_hash::{Algorithm, Context, Sha256};

use crate::input_address::{UnfilledOutput, fill_outputs_from_hash};
use crate::printer::{Sha256Sink, write_derivation_parts, write_sha256_hex};

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

// `Sha256`'s `Ord` compares digest bytes left-to-right, which matches
// base16-lexicographic order (hex encoding is monotonic per byte), so
// `BTreeMap<Sha256, OutputInputs>` gives the correct ATerm key ordering directly.

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

/// Replace each input derivation in `inputs` with its hash modulo, producing
/// the input map of the intermediate derivation directly without building an
/// intermediate `DerivationInputs`.
///
/// `SingleDerivedPath`'s `Ord` groups outputs of the same
/// `Built { drv_path: Opaque(p), .. }` contiguously in ascending `StorePath`
/// order, so caching the last looked-up `StorePath` calls `lookup` once per
/// unique input derivation.
///
/// Returns `Ok(None)` if any input is deferred or dynamic (its output
/// paths are not yet known, so no hash modulo can be computed yet).
fn modulo_drvs<E>(
    inputs: &BTreeSet<SingleDerivedPath>,
    mut lookup: impl FnMut(&StorePath) -> Result<HashModulo, E>,
) -> Result<Option<BTreeMap<Sha256, OutputInputs>>, InputModuloError<E>> {
    let mut drvs: BTreeMap<Sha256, OutputInputs> = BTreeMap::new();
    let mut cached: Option<(&StorePath, HashModulo)> = None;

    for input in inputs {
        let SingleDerivedPath::Built { drv_path, output } = input else {
            continue;
        };
        // Need to build and resolve dynamic derivations first.
        let SingleDerivedPath::Opaque(drv_store_path) = drv_path.as_ref() else {
            return Ok(None);
        };

        let resolved = match &cached {
            Some((prev, hm)) if *prev == drv_store_path => hm,
            _ => {
                let hm = lookup(drv_store_path).map_err(InputModuloError::Lookup)?;
                &cached.insert((drv_store_path, hm)).1
            }
        };

        // Nix merges output names on a hash-key collision
        // (`drvInputs[drvHash].insert(...)` in derivation/masked.cc);
        // replicate that exactly since this is the preimage for output paths.
        match resolved {
            HashModulo::DeferredDrv => return Ok(None),
            // Regular non-CA derivation: replace the derivation path
            // with its hash, keeping the requested output names.
            HashModulo::DrvHash(h) => {
                drvs.entry(*h).or_default().outputs.insert(output.clone());
            }
            // Fixed-output derivation: pretend each output hash is a
            // derivation hash producing a single "out" output.
            HashModulo::CaOutputHashes(output_hashes) => {
                let h = output_hashes.get(output).ok_or_else(|| {
                    InputModuloError::MissingOutputHash {
                        output: output.clone(),
                    }
                })?;
                drvs.entry(*h)
                    .or_default()
                    .outputs
                    .insert(OutputName::default());
            }
        }
    }
    Ok(Some(drvs))
}

/// Substitute the inputs with their hash moduli and stream the
/// intermediate derivation ATerm directly into SHA-256 without cloning
/// the derivation, rebuilding `DerivationInputs`, or buffering the full ATerm.
/// `None` if any input is deferred or dynamic.
fn hash_intermediate<E, O: crate::raw_output::AtermOutput>(
    store_dir: &StoreDir,
    drv: &DerivationT<BTreeSet<SingleDerivedPath>, O>,
    mask_outputs: bool,
    lookup: impl FnMut(&StorePath) -> Result<HashModulo, E>,
) -> Result<Option<Sha256>, InputModuloError<E>> {
    let Some(drvs) = modulo_drvs(&drv.inputs, lookup)? else {
        return Ok(None);
    };
    let srcs = drv.inputs.iter().filter_map(|p| match p {
        SingleDerivedPath::Opaque(path) => Some(path),
        SingleDerivedPath::Built { .. } => None,
    });
    let mut sink = Sha256Sink::new();
    write_derivation_parts(
        store_dir,
        drv,
        srcs,
        &drvs,
        write_sha256_hex,
        mask_outputs,
        &mut sink,
    );
    Ok(Some(sink.finish()))
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
            struct Sha256Fmt(Context);
            impl std::fmt::Write for Sha256Fmt {
                fn write_str(&mut self, s: &str) -> std::fmt::Result {
                    self.0.update(s.as_bytes());
                    Ok(())
                }
            }

            // `{:#x}` renders the bare base16 digest (the non-alternate
            // form prefixes the algorithm, which `method_algorithm()`
            // already provides here).
            let mut w = Sha256Fmt(Context::new(Algorithm::SHA256));
            let dir = store_dir.to_str();
            let sep = if dir.ends_with('/') { "" } else { "/" };
            let _ = write!(
                w,
                "fixed:out:{}:{:#x}:{dir}{sep}{path}",
                ca.method_algorithm(),
                ca.hash().to_owned(),
            );
            let hash =
                w.0.finish()
                    .try_into()
                    .expect("SHA256 context always produces SHA256");
            output_hashes.insert(output_name.clone(), hash);
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

    Ok(match hash_intermediate(store_dir, drv, false, lookup)? {
        Some(h) => HashModulo::DrvHash(h),
        None => HashModulo::DeferredDrv,
    })
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
    Ok(hash_intermediate(store_dir, drv, true, lookup)?)
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
    // The output type already guarantees all outputs are deferred; stream
    // with masked output env vars (the real env keeps its values until the
    // computed paths overwrite them in fill_outputs_from_hash).
    let Some(drv_hash) = hash_intermediate(store_dir, &drv, true, lookup)? else {
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

    /// Nix merges (not replaces) on hash-modulo key collision.
    #[test]
    fn modulo_drvs_collision_merges_like_nix() {
        let h = Sha256::digest(b"same");
        let mut inputs = DerivationInputs::default();
        let mk = |outs: &[&str]| OutputInputs {
            outputs: outs.iter().map(|o| o.parse().unwrap()).collect(),
            dynamic_outputs: BTreeMap::new(),
        };
        inputs.drvs.insert(
            "00000000000000000000000000000000-a.drv".parse().unwrap(),
            mk(&["dev"]),
        );
        inputs.drvs.insert(
            "11111111111111111111111111111111-b.drv".parse().unwrap(),
            mk(&["out"]),
        );
        let flat = BTreeSet::from(&inputs);
        let res = modulo_drvs(&flat, |_| Ok::<_, Infallible>(HashModulo::DrvHash(h)))
            .unwrap()
            .unwrap();
        assert_eq!(res[&h], mk(&["dev", "out"]));
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

    #[test]
    fn multi_output_input_drv_calls_lookup_once() {
        let sd = store_dir();
        let dep: StorePath = "00000000000000000000000000000000-dep.drv".parse().unwrap();
        let dep_arc = std::sync::Arc::new(SingleDerivedPath::Opaque(dep.clone()));
        let mut inputs = BTreeSet::new();
        for out in ["dev", "lib", "out"] {
            inputs.insert(SingleDerivedPath::Built {
                drv_path: dep_arc.clone(),
                output: out.parse().unwrap(),
            });
        }
        let drv = deferred_drv("consumer", inputs);
        let mut calls = 0usize;
        let h = hash_modulo(&sd, &drv, |p| {
            assert_eq!(p, &dep);
            calls += 1;
            Ok::<_, std::convert::Infallible>(HashModulo::DrvHash(Sha256::digest(b"dep")))
        })
        .unwrap();
        assert!(h.is_some());
        assert_eq!(calls, 1);
    }
}
