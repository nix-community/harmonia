{
  pkgs,
  lib,
  crane,
  makeWrapper,
  nix,
  curl,
  nix-src,
}:
let
  craneLib = crane.mkLib pkgs;

  # Extract version from Cargo.toml
  cargoToml = lib.importTOML ./Cargo.toml;
  version = cargoToml.workspace.package.version;

  # Filter source to include only rust-related files
  src = lib.cleanSourceWith {
    src = craneLib.path ./.;
    filter =
      path: type:
      (lib.hasSuffix "\.toml" path)
      || (lib.hasSuffix "\.lock" path)
      || (lib.hasSuffix "\.rs" path)
      || (lib.hasInfix "/harmonia-" path)
      ||
        # Include test keys
        (lib.hasSuffix ".pk" path)
      || (lib.hasSuffix ".sk" path)
      || (lib.hasSuffix ".pem" path)
      || (craneLib.filterCargoSources path type);
  };

  commonArgs = {
    inherit src version;
    pname = "harmonia";
    strictDeps = true;

    # Use mold linker for faster builds on ELF platforms
    stdenv =
      p: if p.stdenv.hostPlatform.isElf then p.stdenvAdapters.useMoldLinker p.stdenv else p.stdenv;
  };

  # Build *just* the cargo dependencies
  cargoArtifacts = craneLib.buildDepsOnly commonArgs;

  # Coverage RUSTFLAGS matching what `cargo llvm-cov show-env` produces
  coverageRustflags = "-C instrument-coverage --cfg=coverage --cfg=trybuild_no_target";

  # Build dependencies with coverage instrumentation for reuse in coverage tests
  cargoArtifactsCov = craneLib.buildDepsOnly (
    commonArgs
    // {
      pnameSuffix = "-cov";
      CARGO_BUILD_RUSTFLAGS = coverageRustflags;
      # Discard profraw from build scripts/proc-macros (sandbox is read-only anyway)
      LLVM_PROFILE_FILE = "/dev/null";
      # Build debug profile to match `cargo build` / `cargo test` defaults
      CARGO_PROFILE = "";
    }
  );

  # Build the actual crate
  harmonia = craneLib.buildPackage (
    commonArgs
    // {
      inherit cargoArtifacts;

      # Add runtime dependencies
      nativeBuildInputs = [ makeWrapper ];

      doCheck = false;

      postInstall = ''
        wrapProgram $out/bin/harmonia \
          --prefix PATH : ${lib.makeBinPath [ nix ]}
      '';

      meta = with lib; {
        description = "Nix binary cache implemented in rust";
        homepage = "https://github.com/nix-community/harmonia";
        license = with licenses; [ mit ];
        maintainers = [ maintainers.mic92 ];
        platforms = platforms.all;
      };
    }
  );

  # Test derivation with coverage - follows nomad pattern
  # https://github.com/nomad/nomad/blob/main/nix/coverage.nix
  # Uses mkCargoDerivation directly since cargoLlvmCov overrides buildPhaseCargoCommand
  tests = craneLib.mkCargoDerivation (
    commonArgs
    // {
      cargoArtifacts = cargoArtifactsCov;
      pnameSuffix = "-llvm-cov";
      doInstallCargoArtifacts = false;

      # Use same RUSTFLAGS as cargoArtifactsCov to avoid rebuilding dependencies
      CARGO_BUILD_RUSTFLAGS = coverageRustflags;

      nativeBuildInputs = [
        nix
        curl
        pkgs.cargo-llvm-cov
        pkgs.jq
      ];

      # Custom build command following nomad pattern:
      # 1. Build binaries with instrumentation (deps already built via cargoArtifactsCov)
      # 2. Run tests with instrumented binaries via env vars
      # 3. Generate report separately with --codecov
      buildPhaseCargoCommand = ''
        export NIX_UPSTREAM_SRC=${nix-src}
        export LLVM_COV=${pkgs.llvmPackages.bintools-unwrapped}/bin/llvm-cov
        export LLVM_PROFDATA=${pkgs.llvmPackages.bintools-unwrapped}/bin/llvm-profdata
        ${lib.optionalString pkgs.stdenv.isDarwin ''
          export _NIX_TEST_NO_SANDBOX="1"
        ''}

        # Set up coverage profile output (but don't override RUSTFLAGS - already set via CARGO_BUILD_RUSTFLAGS)
        export LLVM_PROFILE_FILE="$PWD/target/harmonia-%p-%8m.profraw"
        export CARGO_LLVM_COV=1
        export CARGO_LLVM_COV_TARGET_DIR="$PWD/target"

        # Build workspace binaries with coverage instrumentation
        # Dependencies already built with same flags via cargoArtifactsCov
        cargo build --workspace

        # Point integration tests to instrumented binaries for coverage
        export HARMONIA_DAEMON_BIN="$PWD/target/debug/harmonia-daemon"
        export HARMONIA_CACHE_BIN="$PWD/target/debug/harmonia-cache"

        # Run tests (they will use the instrumented binaries and write profraw data)
        cargo test --workspace

        # Generate coverage report in codecov JSON format
        cargo llvm-cov report --codecov --output-path coverage-raw.json

        # Fix paths: strip build directory prefix to get repo-relative paths
        # e.g., /nix/var/nix/builds/.../source/harmonia-cache/src/foo.rs -> harmonia-cache/src/foo.rs
        # Also filter out stdlib paths (rustc-*/library/...) that leak into coverage
        mkdir -p $out
        jq '
          .coverage |= (
            # First filter out stdlib and other non-project paths
            with_entries(select(.key | test("rustc-.*-src") | not))
            # Then fix paths by extracting repo-relative portion
            | with_entries(.key |= (capture(".*/source/(?<path>.*)") // {path: .}).path)
            # Finally keep only harmonia-* paths (our crates)
            | with_entries(select(.key | startswith("harmonia-")))
          )
        ' coverage-raw.json > $out/${pkgs.stdenv.hostPlatform.system}.json
      '';

      installPhaseCommand = "";
    }
  );

  # Clippy check derivation
  clippy = craneLib.cargoClippy (
    commonArgs
    // {
      inherit cargoArtifacts;
      cargoClippyExtraArgs = "--all-targets --all-features -- -D warnings";
    }
  );
in
{
  inherit harmonia clippy tests;
  default = harmonia;
}
