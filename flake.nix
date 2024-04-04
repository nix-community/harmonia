{
  description = "Nix binary cache implemented in rust using libnix-store";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable-small";
  inputs.flake-parts = {
    url = "github:hercules-ci/flake-parts";
    inputs.nixpkgs-lib.follows = "nixpkgs";
  };
  inputs.treefmt-nix.url = "github:numtide/treefmt-nix";
  inputs.treefmt-nix.inputs.nixpkgs.follows = "nixpkgs";

  nixConfig.extra-substituters = [
    "https://cache.garnix.io"
  ];

  nixConfig.extra-trusted-public-keys = [
    "cache.garnix.io:CTFPyKSLcx5RMJKfLo5EEPUObbA78b0YQ2DTCJXqr9g="
  ];

  outputs = inputs@{ flake-parts, ... }:
    flake-parts.lib.mkFlake { inherit inputs; } {
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
        "x86_64-darwin"
      ];
      imports = [
        inputs.treefmt-nix.flakeModule
      ];
      perSystem = { lib, config, pkgs, ... }: {
        packages.harmonia = pkgs.callPackage ./. { };
        packages.default = config.packages.harmonia;
        checks =
          let
            testArgs = {
              inherit pkgs;
              inherit (inputs) self;
            };
          in
          lib.optionalAttrs pkgs.stdenv.isLinux
            {
              t00-simple = import ./tests/t00-simple.nix testArgs;
              t01-signing = import ./tests/t01-signing.nix testArgs;
              t02-varnish = import ./tests/t02-varnish.nix testArgs;
              t03-chroot = import ./tests/t03-chroot.nix testArgs;
            } // {
            clippy = config.packages.harmonia.override ({
              enableClippy = true;
            });
          };
        devShells.default = pkgs.callPackage ./shell.nix { };

        treefmt = {
          # Used to find the project root
          projectRootFile = "flake.lock";

          programs.rustfmt.enable = true;
          programs.clang-format.enable = true;

          settings.formatter = {
            nix = {
              command = "sh";
              options = [
                "-eucx"
                ''
                  export PATH=${lib.makeBinPath [ pkgs.coreutils pkgs.findutils pkgs.deadnix pkgs.nixpkgs-fmt ]}
                  deadnix --edit "$@"
                  nixpkgs-fmt "$@"
                ''
                "--"
              ];
              includes = [ "*.nix" ];
            };
          };
        };
      };
      flake.nixosModules.harmonia = ./module.nix;
    };
}
