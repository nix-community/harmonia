# Contributing to Harmonia

## Development Environment

Enter the development shell:

```bash
nix develop
```

This provides:
- Rust toolchain (rustc, cargo, rustfmt, clippy)
- cargo-watch, cargo-nextest, cargo-llvm-cov
- rust-analyzer for IDE support
- mold linker on Linux for faster builds

## Building

Build the entire project:

```bash
cargo build --workspace
```

Or use Nix:

```bash
nix build
```

## Running Tests

### Quick iteration with cargo

```bash
# Run all tests with nextest (faster, parallel)
cargo nextest run --workspace

# Run a specific test
cargo nextest run --workspace test_name

# Run tests in a specific crate
cargo nextest run -p harmonia-cache

# Watch mode during development
cargo watch -x 'nextest run --workspace'
```

### Full test suite with Nix

```bash
# Run unit/integration tests with coverage
nix build .#checks.x86_64-linux.tests -L

# Run NixOS VM tests (Linux only)
nix build .#checks.x86_64-linux.nix-daemon -L
nix build .#checks.x86_64-linux.harmonia-daemon -L
```

### Coverage

Coverage reports are generated automatically in CI. To generate locally:

```bash
export LLVM_COV=$(which llvm-cov)
export LLVM_PROFDATA=$(which llvm-profdata)
cargo llvm-cov nextest --workspace --html
# Open target/llvm-cov/html/index.html
```

## Benchmarks

Run the closure download benchmark:

```bash
cargo bench --package harmonia-bench

# With verbose output
cargo bench --package harmonia-bench -- --nocapture
```

This benchmarks downloading a Python closure through harmonia-cache. By default it builds harmonia with the `profiling` profile (release + debug symbols for flamegraphs).

Environment variables:
- `HARMONIA_FLAKE` - Use a nix-built harmonia instead of cargo build
- `BENCH_CLOSURE_FLAKE` - Override the benchmark closure (default: `.#bench-closure`)

View results:

```bash
open target/criterion/closure/download/report/index.html
```

## Code Style

### Formatting

Format all code before committing:

```bash
nix fmt
```

This runs treefmt which handles:
- **Rust**: rustfmt (2024 edition)
- **Nix**: nixfmt
- **C/C++**: clang-format

### Linting

```bash
# Clippy with warnings as errors
cargo clippy --all-targets --all-features -- -D warnings

# Or via Nix (same as CI)
nix build .#clippy -L
```

### Code Guidelines

- No `unsafe` code (enforced via `#![deny(unsafe_code)]` workspace-wide)
- Use `thiserror` for error types
- Use `tokio` for async runtime
- Prefer `log` macros for logging

## Pull Requests

1. Fork and create a feature branch
2. Make your changes
3. Ensure all checks pass:
   ```bash
   nix fmt          # Format code
   cargo clippy --all-targets --all-features -- -D warnings
   cargo nextest run --workspace
   ```
4. Write meaningful commit messages explaining *why*, not just *what*
5. Open a PR against `master`

## CI

PRs are tested by buildbot-nix which runs:
- `nix flake check` (clippy, tests, NixOS VM tests)
- Coverage upload to Codecov

All checks must pass before merging.

## NixOS Module Development

The NixOS module is in `module.nix`. To test changes:

```bash
# Test with nix-daemon backend
nix build .#checks.x86_64-linux.nix-daemon -L

# Test with harmonia-daemon backend
nix build .#checks.x86_64-linux.harmonia-daemon -L
```

## Questions?

Open an issue at https://github.com/nix-community/harmonia/issues
