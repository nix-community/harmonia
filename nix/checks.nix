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
  dependency-diagram =
    let
      generated =
        pkgs.runCommand "generate-dependency-diagram"
          {
            nativeBuildInputs = [
              pkgs.python3
              pkgs.cargo
            ];
            src = packageSet.src;
            script = ../scripts/dependency-diagram.py;
            doc = ../docs/architecture/harmonia-store-structure.md;
          }
          ''
            python3 "$script" \
              --doc "$doc" \
              --manifest-path $src/Cargo.toml \
              > "$out"
          '';
    in
    pkgs.runCommand "check-dependency-diagram"
      {
        nativeBuildInputs = [ pkgs.diffutils ];
      }
      ''
        if ! diff \
          --unified \
          --color=always \
          ${../docs/architecture/harmonia-store-structure.md} \
          ${generated}; then
          echo "Dependency diagram is out of date. Update with:"
          echo "  cp ${generated} docs/architecture/harmonia-store-structure.md"
          exit 1
        fi
        touch $out
      '';
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
