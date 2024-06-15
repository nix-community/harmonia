(import ./lib.nix)
  ({ pkgs, ... }:
  let
    inherit (pkgs) hello;
    copyScript = pkgs.writeShellScript "copy-test" ''
      set -e
      PUBKEY=$(cat $1)
      nix copy \
        --option trusted-public-keys "$PUBKEY" \
        --from http://harmonia:5000 \
        --extra-experimental-features nix-command \
        --to "$2" \
        "$3"
    '';
  in
  {
    name = "t01-signing";

    nodes = {
      harmonia = { ... }: {
        imports = [ ../module.nix ];

        services.harmonia-dev = {
          enable = true;
          signKeyPaths = [ "${./cache.sk}" "${./cache2.sk}" ];
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

      client01.wait_until_succeeds("${copyScript} ${./cache.pk} /root/test-store ${hello}")
      client01.wait_until_succeeds("${copyScript} ${./cache2.pk} /root/test-store2 ${hello}")
      client01.succeed("${hello}/bin/hello --version")
    '';
  })
