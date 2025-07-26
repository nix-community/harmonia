{
  description = "Nix binary cache implemented in rust using libnix-store";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable-small";
  inputs.flake-parts = {
    url = "github:hercules-ci/flake-parts";
    inputs.nixpkgs-lib.follows = "nixpkgs";
  };
  inputs.treefmt-nix.url = "github:numtide/treefmt-nix";
  inputs.treefmt-nix.inputs.nixpkgs.follows = "nixpkgs";
  inputs.crane.url = "github:ipetkov/crane";

  outputs =
    inputs@{ flake-parts, ... }:
    flake-parts.lib.mkFlake { inherit inputs; } {
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
        "x86_64-darwin"
      ];
      imports = [ inputs.treefmt-nix.flakeModule ];
      perSystem =
        {
          lib,
          config,
          pkgs,
          self',
          ...
        }:
        {
          packages.harmonia = pkgs.callPackage ./package.nix {
            crane = inputs.crane;
          };
          packages.default = config.packages.harmonia;
          checks =
            let
              testArgs = {
                inherit pkgs;
                inherit (inputs) self;
              };
              packages = lib.mapAttrs' (n: lib.nameValuePair "package-${n}") self'.packages;
              devShells = lib.mapAttrs' (n: lib.nameValuePair "devShell-${n}") self'.devShells;
            in
            lib.optionalAttrs pkgs.stdenv.isLinux {
              t00-simple = import ./tests/t00-simple.nix testArgs;
              t05-daemon = import ./tests/t05-daemon.nix testArgs;
            }
            // {
              clippy = config.packages.harmonia.override { enableClippy = true; };
            }
            // packages
            // devShells;
          devShells.default = pkgs.callPackage ./shell.nix { };

          treefmt = {
            # Used to find the project root
            projectRootFile = "flake.lock";

            programs.rustfmt.enable = true;
            programs.nixfmt.enable = true;
            programs.deadnix.enable = true;
            programs.clang-format.enable = true;
          };
        };
      flake.nixosModules.harmonia =
        { lib, ... }:
        {
          imports = [
            (lib.modules.importApply ./module.nix { crane = inputs.crane; })
          ];
        };
    };
}
