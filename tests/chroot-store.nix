# Test for serving files from a chroot/custom store location
# This reproduces the issue from https://github.com/nix-community/harmonia/issues/616
#
# The scenario:
# - Nix store files are physically located at a non-standard path (e.g., /chroot/nix/store)
# - The standard Nix daemon uses the normal /nix/store in its database
# - harmonia-cache needs to correctly map between virtual paths (protocol) and real paths (filesystem)
{ pkgs, self }:
pkgs.testers.nixosTest {
  name = "chroot-store";
  globalTimeout = 120;
  nodes = {
    server =
      { pkgs, lib, ... }:
      let
        # We'll set up a chroot-style store at /chroot/nix/store
        # The standard nix-daemon will continue to use /nix/store paths
        chrootBase = "/chroot";
        chrootStore = "${chrootBase}/nix/store";
      in
      {
        imports = [ self.outputs.nixosModules.harmonia ];

        # Use the standard nix-daemon (it uses /nix/store paths)
        # Enable harmonia-cache with virtual/real store mapping
        services.harmonia-dev.cache = {
          enable = true;
          settings = {
            bind = "[::]:5000";
            # Virtual store is /nix/store (what the nix-daemon uses in its database)
            virtual_nix_store = "/nix/store";
            # Real store is where files actually live
            real_nix_store = chrootStore;
          };
        };

        networking.firewall.allowedTCPPorts = [ 5000 ];

        # Ensure we have hello in the normal store first
        system.extraDependencies = [ pkgs.hello ];

        # Set up the chroot store by copying files from the real store
        system.activationScripts.setupChrootStore = lib.stringAfter [ "users" "groups" ] ''
          # Create chroot directory structure
          mkdir -p ${chrootStore}

          # Copy the hello package and its closure to the chroot store location
          echo "Copying ${pkgs.hello} closure to chroot store..."

          # Get the closure of hello and copy each path
          for path in $(${pkgs.nix}/bin/nix-store --query --requisites ${pkgs.hello}); do
            if [ ! -e "${chrootBase}$path" ]; then
              echo "Copying $path to ${chrootBase}$path"
              mkdir -p "$(dirname "${chrootBase}$path")"
              cp -a "$path" "${chrootBase}$path"
            fi
          done

          # Make sure permissions are correct
          chmod -R a+rX ${chrootStore} || true

          echo "Chroot store setup complete"
          ls -la ${chrootStore}/ | head -10
        '';
      };

    client = {
      nix.settings = {
        substituters = [ "http://server:5000" ];
        require-sigs = false;
      };
      nix.extraOptions = ''
        experimental-features = nix-command
      '';
    };
  };

  testScript =
    let
      hashPart = pkg: builtins.substring (builtins.stringLength builtins.storeDir + 1) 32 pkg.outPath;
    in
    ''
      start_all()

      # Wait for harmonia-cache to start
      server.wait_for_unit("harmonia-dev.service")
      server.wait_for_open_port(5000)

      # Verify the chroot store is set up correctly
      server.succeed("test -d /chroot/nix/store")
      server.succeed("test -e /chroot${pkgs.hello}")

      # Show what's in the chroot store
      print("Chroot store contents:")
      print(server.succeed("ls -la /chroot/nix/store/ | head -20"))

      # Verify the files exist in the chroot location
      print("Verifying hello exists in chroot store:")
      print(server.succeed("ls -la /chroot${pkgs.hello}"))

      # Test that the cache info endpoint works
      cache_info = server.succeed("curl -f http://localhost:5000/nix-cache-info")
      print(f"Cache info: {cache_info}")

      # The key test: can we get narinfo for a path in the chroot store?
      # The daemon knows the path exists (in /nix/store), and harmonia should serve it
      # from the real location (/chroot/nix/store)
      hello_hash = "${hashPart pkgs.hello}"
      print(f"Testing narinfo for hello hash: {hello_hash}")

      # This is where the bug manifests - the narinfo request should succeed
      # because the daemon can find the path, but if the chroot mapping is broken,
      # it will return "missed hash"
      narinfo_result = server.succeed(f"curl -s -w '\\n%{{http_code}}' http://localhost:5000/{hello_hash}.narinfo")
      print(f"Narinfo result: {narinfo_result}")

      # Parse the HTTP status code
      lines = narinfo_result.strip().split('\n')
      status_code = lines[-1]
      body = '\n'.join(lines[:-1])

      print(f"HTTP Status: {status_code}")
      print(f"Body: {body}")

      # Check for success
      if status_code != "200":
          # Print harmonia logs for debugging
          print("Harmonia logs:")
          print(server.succeed("journalctl -u harmonia-dev.service --no-pager | tail -50"))
          raise Exception(f"Expected HTTP 200, got {status_code}. Body: {body}")

      # Check that we got a proper narinfo (should contain StorePath)
      assert "StorePath:" in body, f"Response doesn't look like a narinfo: {body}"
      assert "/nix/store/" in body, f"StorePath should use virtual store path: {body}"

      # First, get the NAR URL from the narinfo
      nar_url = None
      for line in body.split('\n'):
          if line.startswith("URL:"):
              nar_url = line.split(":", 1)[1].strip()
              break

      assert nar_url is not None, f"No URL found in narinfo: {body}"
      print(f"NAR URL: {nar_url}")

      # Try to download the NAR
      nar_result = server.succeed(f"curl -s -o /dev/null -w '%{{http_code}}' http://localhost:5000/{nar_url}")
      print(f"NAR download status: {nar_result}")

      assert nar_result.strip() == "200", f"NAR download failed with status {nar_result}"

      # If we got here, the chroot store mapping is working correctly
      # Now test from the client side
      print("Testing client download...")
      client.wait_until_succeeds("timeout 1 curl -f http://server:5000")
      client.succeed("curl -f http://server:5000/nix-cache-info")

      # Create a local store and try to copy from harmonia
      client.succeed("mkdir -p /tmp/test-store")
      client.wait_until_succeeds("nix copy --from http://server:5000/ --to file:///tmp/test-store ${pkgs.hello}")

      # Verify the package was copied
      client.succeed("nix path-info --store file:///tmp/test-store ${pkgs.hello}")
    '';
}
