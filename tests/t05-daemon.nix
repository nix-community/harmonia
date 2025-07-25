{ pkgs, self }:
let
  inherit (pkgs) system;
in
pkgs.nixosTest {
  name = "harmonia-daemon";
  nodes = {
    server = {
      imports = [ self.outputs.nixosModules.harmonia ];
      
      # Enable both harmonia-daemon and harmonia-cache
      services.harmonia-daemon = {
        enable = true;
      };
      
      services.harmonia-dev = {
        enable = true;
        settings = {
          bind = "[::]:5000";
        };
      };

      # Ensure we have some paths in the store for testing
      system.extraDependencies = [ pkgs.hello ];
      
      # Disable the regular nix-daemon to avoid conflicts
      systemd.services.nix-daemon.enable = false;
      systemd.sockets.nix-daemon.enable = false;
      
      networking.firewall.allowedTCPPorts = [ 5000 ];
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

      # Wait for harmonia-daemon to start
      server.wait_for_unit("harmonia-daemon.service")
      server.wait_for_file("/run/harmonia-daemon/socket")
      
      # Wait for harmonia-cache to start
      server.wait_for_unit("harmonia-dev.service")
      server.wait_for_open_port(5000)

      # Test that the socket exists and is accessible
      server.succeed("test -S /run/harmonia-daemon/socket")

      # Test that both services are running
      server.succeed("systemctl is-active harmonia-daemon.service")
      server.succeed("systemctl is-active harmonia-dev.service || (systemctl status harmonia-dev.service; false)")
      
      # Test that harmonia-cache can serve content
      server.succeed("curl -f http://localhost:5000/nix-cache-info")
      
      # Get hello hash and try to fetch narinfo
      hello_hash = "${hashPart pkgs.hello}"
      server.succeed(f"curl -v http://localhost:5000/{hello_hash}.narinfo >&2 || true")

      # Check harmonia-dev logs for the error
      server.succeed("journalctl -u harmonia-dev.service --no-pager >&2")

      # Test that client can fetch from harmonia-cache using nix copy
      client.wait_until_succeeds("timeout 1 curl -f http://server:5000")
      client.succeed("curl -f http://server:5000/nix-cache-info")
      
      # Create a separate store on client
      client.succeed("mkdir -p /tmp/test-store")
      
      # Copy hello package from server's harmonia-cache to the file store
      client.wait_until_succeeds("nix copy --from http://server:5000/ --to file:///tmp/test-store ${pkgs.hello}")
      
      # Verify the package was copied to the file store
      client.succeed("nix path-info --store file:///tmp/test-store ${pkgs.hello}")
      
      # Check logs for any errors
      for service in ["harmonia-daemon", "harmonia-dev"]:
          output = server.succeed(f"journalctl -u {service}.service || true")
          errors = [line for line in output.splitlines() if "error" in line.lower() and "no error" not in line.lower()]
          if errors:
              error_lines = "\n".join(errors[:5])  # Show first 5 errors
              print(f"Found errors in {service} logs (showing first 5):\n{error_lines}")
    '';
}
