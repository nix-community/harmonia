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

      # Set HARMONIA_BIN to the build directory so tests use the release binary
      # Set NIX_UPSTREAM_SRC to nix source for JSON test data
      preCheck = ''
        export HARMONIA_BIN=$(pwd)/target/release
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

  # Test derivation - runs tests with llvm-cov, outputs lcov report
  tests = craneLib.cargoLlvmCov (
    commonArgs
    // {
      inherit cargoArtifacts;
      cargoLlvmCovExtraArgs = "--codecov --output-path $out --remap-path-prefix";
      nativeBuildInputs = [
        nix
        curl
      ];
      preBuild = ''
        export NIX_UPSTREAM_SRC=${nix-src}
        export LLVM_COV=${pkgs.llvmPackages.bintools-unwrapped}/bin/llvm-cov
        export LLVM_PROFDATA=${pkgs.llvmPackages.bintools-unwrapped}/bin/llvm-profdata

        # Build binaries with coverage instrumentation for integration tests
        # Use a SEPARATE target directory to avoid cargo-llvm-cov cleaning them up
        export RUSTFLAGS="-C instrument-coverage --cfg=coverage"
        export LLVM_PROFILE_FILE="$(pwd)/target/llvm-cov-target/harmonia-%p-%m.profraw"
        cargo build --release --bins --target-dir target/harmonia-bins

        # Point tests to our separately-built binaries
        export HARMONIA_BIN=$(pwd)/target/harmonia-bins/release
      ''
      + lib.optionalString pkgs.stdenv.isDarwin ''
        export _NIX_TEST_NO_SANDBOX="1"
      '';
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
