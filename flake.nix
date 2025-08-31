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
          pkgs,
          self',
          ...
        }:
        {
          packages =
            let
              packageSet = pkgs.callPackages ./packages.nix {
                crane = inputs.crane;
              };
            in
            {
              inherit (packageSet) clippy default harmonia;
            };
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
              nix-daemon = import ./tests/nix-daemon.nix testArgs;
              nix-daemon-retry = import ./tests/nix-daemon-retry.nix testArgs;
              harmonia-daemon = import ./tests/harmonia-daemon.nix testArgs;
            }
            // packages
            // devShells;
          devShells.default = pkgs.callPackage ./devShell.nix { };

          treefmt = {
            # Used to find the project root
            projectRootFile = "flake.lock";

            programs.rustfmt = {
              enable = true;
              edition = "2024";
            };
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
