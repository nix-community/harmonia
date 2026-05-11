# Fuzzing

libFuzzer harnesses for harmonia parsers, run via `cargo-fuzz`.

## Targets

- `aterm_parse` — `harmonia_store_aterm::parse_derivation_aterm` on arbitrary
  UTF-8 input.
- `nar_parse` — `harmonia_nar::archive::read_nar` on arbitrary bytes.
- `protocol_request` — `daemon::wire::types2::Request` deserialization via
  `NixReader` on arbitrary bytes.

## Running

With stable Rust (no sanitizer; catches panics, infinite loops, OOMs):

```sh
nix shell nixpkgs#cargo-fuzz nixpkgs#cargo nixpkgs#rustc
cd fuzz
cargo fuzz run -s none aterm_parse
cargo fuzz run -s none nar_parse
cargo fuzz run -s none protocol_request
```

With nightly Rust + AddressSanitizer (also catches memory bugs in unsafe/FFI):

```sh
nix shell nixpkgs#cargo-fuzz "github:nix-community/fenix#minimal.toolchain"
cd fuzz
cargo fuzz run aterm_parse
```

Crashes are written to `fuzz/artifacts/<target>/`. Reproduce with:

```sh
cargo fuzz run -s none <target> artifacts/<target>/<crash-file>
```

## Corpus

Seed inputs live under `fuzz/corpus/<target>/`. Add representative inputs there
to improve coverage. `cargo fuzz` auto-discovers them.
