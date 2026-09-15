{
  description = "Nix binary cache implemented in rust using libnix-store";

  inputs.nixpkgs.url = "git+https://github.com/NixOS/nixpkgs?shallow=1&ref=nixpkgs-unstable";
  inputs.crane.url = "github:ipetkov/crane";
  inputs.nix = {
    url = "github:nixos/nix";
    # We just need some test data, we're not building upstream nix.
    flake = false;
  };

  outputs =
    {
      self,
      nixpkgs,
      crane,
      nix,
    }:
    let
      inherit (nixpkgs) lib;

      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
      ];

      eachSystem =
        f:
        lib.genAttrs systems (
          system:
          f {
            inherit system;
            pkgs = nixpkgs.legacyPackages.${system};
          }
        );

      packageSet = eachSystem (
        { pkgs, ... }:
        pkgs.callPackages ./nix/packages.nix {
          inherit crane;
          nix-src = nix;
        }
      );

      treefmt = eachSystem ({ pkgs, ... }: pkgs.callPackage ./nix/treefmt.nix { });
    in
    {
      packages = eachSystem (
        { system, ... }:
        {
          inherit (packageSet.${system}) harmonia;
          default = packageSet.${system}.harmonia;
        }
      );

      checks = eachSystem (
        { pkgs, system }:
        import ./nix/checks.nix {
          inherit
            self
            lib
            pkgs
            system
            ;
          packageSet = packageSet.${system};
          treefmt = treefmt.${system};
        }
      );

      devShells = eachSystem (
        { pkgs, ... }:
        {
          default = pkgs.callPackage ./nix/devShell.nix { nix-src = nix; };
        }
      );

      formatter = eachSystem ({ system, ... }: treefmt.${system});

      nixosModules.harmonia =
        { lib, ... }:
        {
          imports = [
            (lib.modules.importApply ./nix/module.nix {
              inherit crane;
              nix-src = nix;
            })
          ];
        };

      herculesCI = import ./nix/herculesCI.nix {
        inherit self lib systems;
        pkgs = nixpkgs.legacyPackages.x86_64-linux;
      };
    };
}
