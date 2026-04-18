{
  self,
  lib,
  pkgs,
  system,
  packageSet,
  treefmt,
}:
let
  testArgs = {
    inherit pkgs self;
  };
  packages = lib.mapAttrs' (n: lib.nameValuePair "package-${n}") self.packages.${system};
  devShells = lib.mapAttrs' (n: lib.nameValuePair "devShell-${n}") self.devShells.${system};
in
{
  inherit (packageSet) tests clippy;
  treefmt = treefmt.config.build.check self;
  # Benchmark closure - a decent-sized Python environment for download benchmarks.
  # Built in CI so the bench job can substitute it instead of building locally.
  bench-closure = pkgs.python3.withPackages (
    ps: with ps; [
      numpy
      pandas
      requests
    ]
  );
}
// lib.optionalAttrs pkgs.stdenv.isLinux {
  nix-daemon = import ./tests/nix-daemon.nix testArgs;
  harmonia-daemon = import ./tests/harmonia-daemon.nix testArgs;
  chroot-store = import ./tests/chroot-store.nix testArgs;
}
// packages
// devShells
