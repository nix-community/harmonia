{
  pkgs,
  nix-src,
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
      cargo-llvm-cov
      cargo-nextest
      # LLVM tools needed for cargo-llvm-cov
      llvmPackages.bintools-unwrapped
      # compress-tools-rs links against libarchive via pkg-config
      pkg-config
    ];

    buildInputs = with pkgs; [
      rust-analyzer
      rustfmt
      clippy
      libarchive
    ];

    # provide a dummy configuration for testing
    CONFIG_FILE = pkgs.writeText "config.toml" "";

    RUST_SRC_PATH = "${pkgs.rust.packages.stable.rustPlatform.rustLibSrc}";

    # Path to upstream Nix source for JSON test data
    NIX_UPSTREAM_SRC = nix-src;
  }
