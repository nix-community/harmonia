# Changelog

## [Unreleased]

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

### Added

- `UnkeyedRealisation` type with `out_path` and `signatures` fields.

## [0.0.0-alpha.0]

Initial pre-release.
