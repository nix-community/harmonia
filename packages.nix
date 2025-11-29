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

  # Build the actual crate
  harmonia = craneLib.buildPackage (
    commonArgs
    // {
      inherit cargoArtifacts;

      # Add runtime dependencies
      nativeBuildInputs = [ makeWrapper ];

      doCheck = true;
      nativeCheckInputs = [
        nix
        curl
      ];

      # Set NIX_UPSTREAM_SRC to nix source for JSON test data
      # Tests use CARGO_BIN_EXE_* to find binaries automatically
      preCheck = ''
        export NIX_UPSTREAM_SRC=${nix-src}
      ''
      + lib.optionalString pkgs.stdenv.isDarwin ''
        export _NIX_TEST_NO_SANDBOX="1"
      '';

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
      inherit cargoArtifacts;
      pnameSuffix = "-llvm-cov";
      doInstallCargoArtifacts = false;

      nativeBuildInputs = [
        nix
        curl
        pkgs.cargo-llvm-cov
        pkgs.jq
      ];

      # Custom build command following nomad pattern:
      # 1. Run tests with --no-report to collect coverage data
      # 2. Generate report separately with --codecov
      buildPhaseCargoCommand = ''
        export NIX_UPSTREAM_SRC=${nix-src}
        export LLVM_COV=${pkgs.llvmPackages.bintools-unwrapped}/bin/llvm-cov
        export LLVM_PROFDATA=${pkgs.llvmPackages.bintools-unwrapped}/bin/llvm-profdata
        ${lib.optionalString pkgs.stdenv.isDarwin ''
          export _NIX_TEST_NO_SANDBOX="1"
        ''}

        # Run tests and collect coverage data (no report yet)
        cargo llvm-cov --no-report --workspace

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
