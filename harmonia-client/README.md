# harmonia-client

Command-line interface wrapper for Nix.

## Overview

This crate provides the `harmonia` CLI binary, which wraps the `nix` command with experimental features enabled by default.

## Usage

```bash
# Equivalent to: nix --experimental-features "nix-command flakes" build
harmonia build .#package

# All arguments are passed through to nix
harmonia flake show
harmonia develop
```

## Key Characteristics

- **Thin wrapper**: Passes all arguments to `nix` command
- **Enables experimental features**: Automatically enables `nix-command` and `flakes`
- **Simple**: Minimal code, just command delegation
