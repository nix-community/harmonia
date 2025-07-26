{
  pkgs,
  lib,
  crane,
  makeWrapper,
  openssl,
  nix,
  curl,
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

    nativeBuildInputs = [
      pkg-config
    ];

    buildInputs = [
      openssl
    ];
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
      preCheck = ''
        export HARMONIA_BIN=$(pwd)/target/release
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
  inherit harmonia clippy;
  default = harmonia;
}
