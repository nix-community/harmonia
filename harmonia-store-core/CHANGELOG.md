# Changelog

## [Unreleased]

Various changes, and supporting the (at this time) latest Nix release, 2.34.

### Added

- `BasicDerivation` JSON serialization and deserialization, using a flat store-path array for inputs (vs. `Derivation`'s nested `{srcs, drvs}` format).
- `StorePath::to_absolute_path` method for combining with a `StoreDir` into a native `PathBuf`.
- `Realisation::fingerprint` and `Realisation::sign` methods for realisation signing.
- `DerivationT::map_inputs` for transforming derivation inputs while keeping everything else the same.
- `DerivationT::apply_rewrites` for substituting placeholder strings in builder, args, env, and structured attrs.
- `Derivation::try_resolve` for resolving input derivation references into concrete store paths, producing a `BasicDerivation`.
- `DerivationInputs` now implements `From<&StorePathSet>`.
- Re-exported `ParseContentAddressError` from the `store_path` module.

### Changed

- `Realisation::dependent_realisations` is now always empty (Nix hardcodes `"dependentRealisations": {}` for backwards compat).

## [0.0.0-alpha.0]

Initial pre-release.
