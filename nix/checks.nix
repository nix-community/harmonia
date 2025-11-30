{
  self,
  lib,
  pkgs,
  system,
  packageSet,
  treefmt,
}:
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
// lib.optionalAttrs pkgs.stdenv.isLinux (
  lib.mapAttrs' (
    name: _:
    lib.nameValuePair (lib.removeSuffix ".nix" name) (
      import (./tests + "/${name}") { inherit pkgs self; }
    )
  ) (builtins.readDir ./tests)
)
// lib.mapAttrs' (n: lib.nameValuePair "package-${n}") self.packages.${system}
// lib.mapAttrs' (n: lib.nameValuePair "devShell-${n}") self.devShells.${system}
