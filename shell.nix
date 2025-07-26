{
  pkgs ?
    (builtins.getFlake (builtins.toString ./.)).inputs.nixpkgs.legacyPackages.${builtins.currentSystem},
  clippy ? pkgs.clippy,
  libsodium ? pkgs.libsodium,
  openssl ? pkgs.openssl,
  rust-analyzer ? pkgs.rust-analyzer,
  rustfmt ? pkgs.rustfmt,
}:

pkgs.mkShell {
  name = "harmonia";
  nativeBuildInputs = with pkgs; [
    rustc
    cargo
    cargo-watch
    pkg-config
  ] ++ pkgs.lib.optionals pkgs.stdenv.isLinux [
    clang
    mold
  ];
  buildInputs = [
    libsodium
    rustfmt
    clippy
    openssl
    rust-analyzer
  ];

  # provide a dummy configuration for testing
  CONFIG_FILE = pkgs.writeText "config.toml" "";

  RUST_SRC_PATH = "${pkgs.rust.packages.stable.rustPlatform.rustLibSrc}";

  # Use clang and mold linker for faster builds on Linux
  RUSTFLAGS = pkgs.lib.optionalString pkgs.stdenv.isLinux "-C link-arg=-fuse-ld=mold -C linker=clang";
}
