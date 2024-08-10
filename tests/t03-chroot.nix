(import ./lib.nix) (
  { ... }:
  {
    name = "t03-chroot";

    nodes = {
      harmonia =
        { pkgs, ... }:
        {
          imports = [ ../module.nix ];

          services.harmonia-dev.enable = true;
          # We need to manipulate the target store first
          systemd.services."harmonia-dev".wantedBy = pkgs.lib.mkForce [ ];

          networking.firewall.allowedTCPPorts = [ 5000 ];
          nix.settings.store = "/guest?read-only=1";
          nix.extraOptions = ''
            experimental-features = nix-command read-only-local-store
          '';
        };

      client01 =
        { lib, ... }:
        {
          nix.settings.require-sigs = false;
          nix.settings.substituters = lib.mkForce [ "http://harmonia:5000" ];
          nix.extraOptions = ''
            experimental-features = nix-command
          '';
        };
    };

    testScript = ''
      import json
      start_all()

      harmonia.wait_until_succeeds("echo 'test contents' > /my-file")
      harmonia.wait_until_succeeds("mkdir /my-dir && cp /my-file /my-dir/")
      f = harmonia.wait_until_succeeds("nix store --store /guest add-file /my-file")
      d = harmonia.wait_until_succeeds("nix store --store /guest add-path /my-dir")
      harmonia.systemctl("start harmonia-dev.service")
      harmonia.wait_for_unit("harmonia-dev.service")

      client01.wait_until_succeeds("curl -f http://harmonia:5000/version")
      client01.succeed("curl -f http://harmonia:5000/nix-cache-info")

      client01.wait_until_succeeds(f"nix copy --from http://harmonia:5000/ {f}")
      client01.succeed(f"grep 'test contents' {f}")

      dhash = d.removeprefix("/nix/store/")
      dhash = dhash[:dhash.find('-')]
      out = client01.wait_until_succeeds(f"curl -v http://harmonia:5000/{dhash}.ls")
      data = json.loads(out)
      print(out)
      assert data["version"] == 1, "version is not correct"
      assert data["root"]["entries"]["my-file"]["type"] == "regular", "expect my-file file in listing"

      out = client01.wait_until_succeeds(f"curl -v http://harmonia:5000/serve/{dhash}/")
      print(out)
      assert "my-file" in out, "my-file not in listing"

      out = client01.wait_until_succeeds(f"curl -v http://harmonia:5000/serve/{dhash}/my-file").strip()
      print(out)
      assert "test contents" == out, f"expected 'test contents', got '{out}'"
    '';
  }
)
