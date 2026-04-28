# Changelog

## [Unreleased]

Various changes, and supporting the latest Nix `master` branch.

### Breaking Changes

#### Types

- `Realisation` restructured to `{ key: DrvOutput, value: UnkeyedRealisation }`, matching upstream Nix's key-value composition.
  The old flat fields and `dependent_realisations` are removed.
- `DrvOutput` now uses `StorePath` (not `Hash`) for `drv_path`, and uses `^` as the separator in its string format.
- `BuildResult::built_outputs` changed from `BTreeMap<DrvOutput, Realisation>` to `BTreeMap<OutputName, UnkeyedRealisation>`, since `built_outputs` is per-derivation and only the output name varies.
  The `DrvOutputs` type alias was removed.

#### JSON formats

- `Signature` changed from `"name:base64sig"` strings to `{"keyName": "...", "sig": "..."}` objects.
  A `RawSignature` newtype handles base64 encoding/decoding of the signature bytes.
- Path-info switched to V3 format with structured signatures.
- Build-result switched to structured signatures in `built_outputs`.

### Fixed

- Content-addressed fixed-output store path computation.

### Security

- `SecretKey` bytes are now compared in constant time.
- `SecretKey` is redacted in `Debug` output and key buffers are zeroized on drop.

### Added

- `BasicDerivation` JSON serialization and deserialization, using a flat store-path array for inputs (vs. `Derivation`'s nested `{srcs, drvs}` format).
- `UnkeyedRealisation` type with `out_path` and `signatures` fields.
- `StorePath::to_absolute_path` method for combining with a `StoreDir` into a native `PathBuf`.
- `Realisation::fingerprint` and `Realisation::sign` methods for realisation signing.
- `DerivationT::map_inputs` for transforming derivation inputs while keeping everything else the same.
- `DerivationT::apply_rewrites` for substituting placeholder strings in builder, args, env, and structured attrs.
- `Derivation::try_resolve` for resolving input derivation references into concrete store paths, producing a `BasicDerivation`.
- `DerivationInputs` now implements `From<&StorePathSet>`.
- Re-exported `ParseContentAddressError` from the `store_path` module.

### Changed

- Replaced `ring` with `ed25519-dalek`/`getrandom` for signing.
- Dropped `serde_with` in favour of a small `impl_serde_via_string!` macro.
- `Realisation::dependent_realisations` is now always empty (Nix hardcodes `"dependentRealisations": {}` for backwards compat).

## [0.0.0-alpha.0]

Initial pre-release.
