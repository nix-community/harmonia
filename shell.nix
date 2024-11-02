{
  pkgs ?
    (builtins.getFlake (builtins.toString ./.)).inputs.nixpkgs.legacyPackages.${builtins.currentSystem},
  boost ? pkgs.boost,
  clippy ? pkgs.clippy,
  lib ? pkgs.lib,
  libiconv ? pkgs.libiconv,
  libsodium ? pkgs.libsodium,
  nixVersions ? pkgs.nixVersions,
  nlohmann_json ? pkgs.nlohmann_json,
  openssl ? pkgs.openssl,
  rust-analyzer ? pkgs.rust-analyzer,
  rustfmt ? pkgs.rustfmt,
  stdenv ? pkgs.stdenv,
}:

pkgs.mkShell {
  name = "harmonia";
  nativeBuildInputs = with pkgs; [
    rustc
    cargo
    cargo-watch
    pkg-config
  ];
  buildInputs = [
    nixVersions.latest
    nlohmann_json
    libsodium
    boost
    rustfmt
    clippy
    openssl
    rust-analyzer
  ] ++ lib.optional (stdenv.isDarwin) [ libiconv ];

  # provide a dummy configuration for testing
  CONFIG_FILE = pkgs.writeText "config.toml" "";

  RUST_SRC_PATH = "${pkgs.rust.packages.stable.rustPlatform.rustLibSrc}";
}
