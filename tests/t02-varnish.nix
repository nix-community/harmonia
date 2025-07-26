(import ./lib.nix) (
  { pkgs, ... }:
  {
    name = "t02-varnish";

    nodes = {
      harmonia =
        { pkgs, inputs, ... }:
        let
          sock = "/run/harmonia/socket";
        in
        {
          imports = [ inputs.self.nixosModules.harmonia ];

          services.harmonia-dev.cache.enable = true;
          services.harmonia-dev.cache.settings.bind = "unix:${sock}";

          services.varnish = {
            enable = true;
            http_address = "0.0.0.0:80";
            config = ''
              vcl 4.1;
              backend harmonia {
                .path = "${sock}";
              }
            '';
          };

          networking.firewall.allowedTCPPorts = [ 80 ];
          environment.systemPackages = [ pkgs.hello ];
        };

      client01 =
        { lib, ... }:
        {
          nix.settings.require-sigs = false;
          nix.settings.substituters = lib.mkForce [ "http://harmonia" ];
          nix.extraOptions = ''
            experimental-features = nix-command
          '';
        };
    };

    testScript = ''
      start_all()

      client01.wait_until_succeeds("timeout 1 curl -f http://harmonia/version")
      client01.succeed("curl -f http://harmonia/nix-cache-info")

      client01.wait_until_succeeds("nix copy --from http://harmonia/ ${pkgs.hello}")
      client01.succeed("${pkgs.hello}/bin/hello")
    '';
  }
)
