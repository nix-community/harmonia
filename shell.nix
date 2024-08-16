{
  pkgs ?
    (builtins.getFlake (builtins.toString ./.)).inputs.nixpkgs.legacyPackages.${builtins.currentSystem},
  boost ? pkgs.boost,
  cargo-audit ? pkgs.cargo-audit,
  cargo-edit ? pkgs.cargo-edit,
  cargo-outdated ? pkgs.cargo-outdated,
  cargo-watch ? pkgs.cargo-watch,
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
    pkg-config
  ];
  buildInputs = [
    nixVersions.latest
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
    rust-analyzer
  ] ++ lib.optional (stdenv.isDarwin) [ libiconv ];

  # provide a dummy configuration for testing
  CONFIG_FILE = pkgs.writeText "config.toml" "";

  RUST_SRC_PATH = "${pkgs.rust.packages.stable.rustPlatform.rustLibSrc}";
}
