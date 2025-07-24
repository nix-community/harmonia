{
  pkgs ?
    (builtins.getFlake (builtins.toString ./.)).inputs.nixpkgs.legacyPackages.${builtins.currentSystem},
  rustPlatform ? pkgs.rustPlatform,
  nix-gitignore ? pkgs.nix-gitignore,
  lib ? pkgs.lib,
  clippy ? pkgs.clippy,
  pkg-config ? pkgs.pkg-config,
  libsodium ? pkgs.libsodium,
  openssl ? pkgs.openssl,
  nix ? pkgs.nix,
  makeWrapper ? pkgs.makeWrapper,
  enableClippy ? false,
}:

rustPlatform.buildRustPackage (
  {
    pname = "harmonia";
    version = "2.1.0";
    src = nix-gitignore.gitignoreSource [ ] (
      lib.sources.sourceFilesBySuffices (lib.cleanSource ./.) [
        ".rs"
        ".toml"
        ".lock"
        ".cpp"
        ".h"
        ".md"
      ]
    );
    cargoLock.lockFile = ./Cargo.lock;

    nativeBuildInputs = [
      pkg-config
      makeWrapper
    ] ++ lib.optionals enableClippy [ clippy ];
    buildInputs = [
      libsodium
      openssl
    ];
    doCheck = false;

    postInstall = ''
      wrapProgram $out/bin/harmonia \
        --prefix PATH : ${lib.makeBinPath [ nix ]}
    '';

    meta = with lib; {
      description = "Nix binary cache implemented in rust";
      homepage = "https://github.com/nix-community/harmonia";
      license = with licenses; [ mit ];
      maintainers = [ maintainers.conni2461 ];
      platforms = platforms.all;
    };
  }
  // lib.optionalAttrs enableClippy {
    buildPhase = ''
      cargo clippy --all-targets --all-features -- -D warnings
    '';
    installPhase = ''
      touch $out
    '';
  }
)
