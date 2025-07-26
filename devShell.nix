{
  pkgs,
}:
(pkgs.mkShell.override {
  stdenv =
    if pkgs.stdenv.hostPlatform.isElf then
      pkgs.stdenvAdapters.useMoldLinker pkgs.stdenv
    else
      pkgs.stdenv;
})
  {
    nativeBuildInputs = with pkgs; [
      rustc
      cargo
      cargo-watch
      pkg-config
    ];

    buildInputs = with pkgs; [
      libsodium
      openssl
      rust-analyzer
      rustfmt
      clippy
    ];

    # provide a dummy configuration for testing
    CONFIG_FILE = pkgs.writeText "config.toml" "";

    RUST_SRC_PATH = "${pkgs.rust.packages.stable.rustPlatform.rustLibSrc}";
  }
