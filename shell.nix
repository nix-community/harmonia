{ pkgs ? import <nixpkgs> { }
, nixVersions ? pkgs.nixVersions
, nlohmann_json ? pkgs.nlohmann_json
, libsodium ? pkgs.libsodium
, boost ? pkgs.boost
, rustfmt ? pkgs.rustfmt
, clippy ? pkgs.clippy
, cargo-watch ? pkgs.cargo-watch
, cargo-edit ? pkgs.cargo-edit
, cargo-outdated ? pkgs.cargo-outdated
, cargo-audit ? pkgs.cargo-audit
, openssl ? pkgs.openssl
}:

pkgs.mkShell {
  name = "harmonia";
  nativeBuildInputs = with pkgs; [ rustc cargo pkg-config ];
  buildInputs = [
    nixVersions.unstable
    nlohmann_json
    libsodium
    boost
    rustfmt
    clippy
    cargo-watch
    cargo-edit
    cargo-outdated
    cargo-audit
    openssl
  ];

  # provide a dummy configuration for testing
  CONFIG_FILE = pkgs.writeText "config.toml" "";

  RUST_SRC_PATH = "${pkgs.rust.packages.stable.rustPlatform.rustLibSrc}";
}
