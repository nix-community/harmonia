{
  description = "Nix binary cache implemented in rust using libnix-store";

  inputs.nixpkgs.url = "git+https://github.com/NixOS/nixpkgs?shallow=1&ref=nixpkgs-unstable";
  inputs.crane.url = "github:ipetkov/crane";
  inputs.nix-darwin.url = "github:nix-darwin/nix-darwin";
  inputs.nix-darwin.inputs.nixpkgs.follows = "nixpkgs";
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
      nix-darwin,
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

      # Its tests fail with cargo 1.99:
      # https://github.com/nix-community/harmonia/issues/1278
      cargo-llvm-cov = pkgs: pkgs.cargo-llvm-cov.overrideAttrs { doCheck = false; };

      packageSet = eachSystem (
        { pkgs, ... }:
        pkgs.callPackages ./nix/packages.nix {
          inherit crane;
          nix-src = nix;
          cargo-llvm-cov = cargo-llvm-cov pkgs;
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
        // lib.optionalAttrs (lib.hasSuffix "-darwin" system) {
          darwin-module =
            (nix-darwin.lib.darwinSystem {
              inherit system;
              modules = [
                self.darwinModules.harmonia
                {
                  system.stateVersion = 5;
                  services.harmonia-dev.gc = {
                    enable = true;
                    automatic = true;
                    deleteOlderThan = "30d";
                    ensureFree = "20%";
                    gcRootsDirs = [ "/mnt/extra-roots" ];
                  };
                }
              ];
            }).system;
        }
      );

      devShells = eachSystem (
        { pkgs, ... }:
        {
          default = pkgs.callPackage ./nix/devShell.nix {
            nix-src = nix;
            cargo-llvm-cov = cargo-llvm-cov pkgs;
          };
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

      darwinModules.harmonia = lib.modules.importApply ./nix/darwin-module.nix {
        inherit crane;
        nix-src = nix;
      };

      herculesCI = import ./nix/herculesCI.nix {
        inherit self lib systems;
        pkgs = nixpkgs.legacyPackages.x86_64-linux;
      };
    };
}
