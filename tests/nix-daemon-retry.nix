(import ./lib.nix) (
  { pkgs, ... }:
  let
    testSmall = pkgs.runCommand "test-small" { } ''
      dd if=/dev/urandom of=$out bs=1 count=1
    '';
    testBig = pkgs.runCommand "test-big" { } ''
      dd if=/dev/urandom of=$out bs=1M count=5
    '';
  in
  {
    name = "nix-daemon";

    nodes = {
      harmonia =
        { pkgs, inputs, ... }:
        {
          imports = [ inputs.self.nixosModules.harmonia ];

          services.harmonia-dev.cache.enable = true;
          services.harmonia-dev.cache.signKeyPaths = [ "${./cache.sk}" ];

          networking.firewall.allowedTCPPorts = [ 5000 ];
          system.extraDependencies = [
            pkgs.hello
            testSmall
            testBig
          ];
        };

      flaky-proxy =
        { ... }:
        {
          environment.systemPackages = [
            pkgs.socat
          ];
          networking.firewall.allowedTCPPorts = [ 5001 ];

          systemd.services.flaky-proxy = {
            description = "A proxy that disconnects after a certain number of bytes";
            wantedBy = [ "multi-user.target" ];
            after = [ "network-online.target" ];
            wants = [ "network-online.target" ];
            serviceConfig = {
              ExecStart = ''
                ${pkgs.socat}/bin/socat TCP-LISTEN:5001,fork SYSTEM:"${pkgs.socat}/bin/socat - TCP\\:harmonia\\:5000 | head -c 1048576"
              '';
              Restart = "always";
            };
          };
        };

      client01 =
        { lib, ... }:
        {
          nix.settings.require-sigs = false;
          # This client exclusively uses the proxy as its substituter
          nix.settings.substituters = lib.mkForce [ "http://flaky-proxy:5001" ];
          nix.settings.download-attempts = 10; # 5 MB, 1 MB at a time, should take about 5 attempts
          nix.extraOptions = ''
            experimental-features = nix-command
          '';
        };
    };

    testScript = ''
      start_all()

      client01.wait_until_succeeds("timeout 10 curl http://flaky-proxy:5001")
      client01.succeed("curl -f http://flaky-proxy:5001/nix-cache-info")

      client01.wait_until_succeeds("nix copy --from http://flaky-proxy:5001/ ${testSmall}")
      client01.succeed("nix copy --from http://flaky-proxy:5001/ ${testBig}")
    '';
  }
)
