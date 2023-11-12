(import ./lib.nix)
  ({ pkgs, ... }:
  let
    inherit (pkgs) hello;
    copyScript = pkgs.writeShellScript "copy-test" ''
      set -e
      PUBKEY=$(cat ${./cache.pk})
      nix copy \
        --option trusted-public-keys "$PUBKEY" \
        --from http://harmonia:5000 \
        --extra-experimental-features nix-command \
        --to /root/test-store \
        "$@"
    '';
  in
  {
    name = "t01-signing";

    nodes = {
      harmonia = { ... }: {
        imports = [ ../module.nix ];

        services.harmonia-dev = {
          enable = true;
          signKeyPath = "${./cache.sk}";
        };

        networking.firewall.allowedTCPPorts = [ 5000 ];
        system.extraDependencies = [ hello ];
      };

      client01 = { lib, ... }: {
        nix.settings.substituters = lib.mkForce [ "http://harmonia:5000" ];
      };
    };

    testScript = ''
      start_all()

      client01.wait_until_succeeds("curl -f http://harmonia:5000/version")
      client01.succeed("curl -f http://harmonia:5000/nix-cache-info")

      client01.wait_until_succeeds("${copyScript} ${hello}")
      client01.succeed("${hello}/bin/hello --version")
    '';
  })
