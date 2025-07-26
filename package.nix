{
  pkgs,
  lib,
  crane,
  enableClippy ? false,
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

    nativeBuildInputs = with pkgs; [
      pkg-config
    ];

    buildInputs = with pkgs; [
      libsodium
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
      nativeBuildInputs = commonArgs.nativeBuildInputs ++ [ pkgs.makeWrapper ];

      doCheck = true;
      nativeCheckInputs = with pkgs; [
        nix
        curl
      ];

      postInstall = ''
        wrapProgram $out/bin/harmonia \
          --prefix PATH : ${lib.makeBinPath [ pkgs.nix ]}
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
  harmoniaClippy = craneLib.cargoClippy (
    commonArgs
    // {
      inherit cargoArtifacts;
      cargoClippyExtraArgs = "--all-targets --all-features -- -D warnings";
    }
  );
in
if enableClippy then harmoniaClippy else harmonia
