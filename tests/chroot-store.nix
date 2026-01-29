{ pkgs, self }:
pkgs.testers.nixosTest {
  name = "chroot-store";
  nodes = {
    harmonia =
      { ... }:
      {
        imports = [ self.outputs.nixosModules.harmonia ];

        services.harmonia-dev.cache = {
          enable = true;
          settings = {
            real_nix_store = "/guest/nix/store";
            virtual_nix_store = "/nix/store";
          };
        };

        networking.firewall.allowedTCPPorts = [ 5000 ];

        system.activationScripts.setupChroot = ''
          mkdir -p /guest/nix/store
          chmod 755 /guest/nix/store
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

    # Create a dummy file and add it to the SYSTEM store (so daemon knows it)
    harmonia.succeed("echo 'test contents' > /my-file")
    f = harmonia.succeed("nix --extra-experimental-features nix-command store add-file /my-file").strip()

    # Also create a directory
    harmonia.succeed("mkdir /my-dir && cp /my-file /my-dir/file")
    d = harmonia.succeed("nix --extra-experimental-features nix-command store add-path /my-dir").strip()

    # Now copy these paths to the chroot store where harmonia will look for them
    harmonia.succeed(f"cp -a {f} /guest{f}")
    harmonia.succeed(f"cp -a {d} /guest{d}")

    # Wait for harmonia
    harmonia.wait_for_unit("harmonia-dev.service")
    harmonia.wait_for_open_port(5000)

    # Client checks
    client01.wait_until_succeeds("timeout 1 curl -f http://harmonia:5000/version")

    # Check if client can fetch the file
    client01.wait_until_succeeds(f"nix copy --from http://harmonia:5000/ {f}")
    client01.succeed(f"grep 'test contents' {f}")

    # Check listing logic
    dhash = d.removeprefix("/nix/store/")
    dhash = dhash[:dhash.find('-')]

    out = client01.wait_until_succeeds(f"curl -sf http://harmonia:5000/{dhash}.ls")
    data = json.loads(out)
    assert data["version"] == 1, "version is not correct"
    assert data["root"]["entries"]["file"]["type"] == "regular", "expect file in listing"

    # Verify serve endpoint works
    out = client01.wait_until_succeeds(f"curl -sf http://harmonia:5000/serve/{dhash}/file").strip()
    assert "test contents" == out, f"expected 'test contents', got '{out}'"
  '';
}
