{ pkgs, self }:
pkgs.testers.nixosTest {
  name = "sandbox";
  globalTimeout = 120;
  nodes.machine = {
    imports = [ self.outputs.nixosModules.harmonia ];

    services.harmonia-dev.daemon = {
      enable = true;
    };

    # Disable the regular nix-daemon to avoid conflicts
    systemd.services.nix-daemon.enable = false;
    systemd.sockets.nix-daemon.enable = false;

    # Ensure /nix/var/nix/builds exists for the build sandbox
    systemd.tmpfiles.rules = [
      "d /nix/var/nix/builds 0755 root root -"
      "d /nix/var/nix/userpool 0755 root root -"
    ];
  };

  testScript = ''
    machine.wait_for_unit("multi-user.target")
    machine.wait_for_unit("harmonia-daemon.service")
    machine.wait_for_file("/run/harmonia-daemon/socket")

    # Verify harmonia-daemon is healthy
    machine.succeed("systemctl is-active harmonia-daemon.service")

    # Trigger a real build through the daemon socket.
    # The daemon runs as root, so it auto-allocates build UIDs and
    # uses user namespaces for isolation.
    drv_path = machine.succeed(
        "nix-instantiate -E '(derivation { name = \"id-test\"; builder = \"/bin/sh\";"
        " args = [\"-c\" \"echo hello > $out\"]; system = \"x86_64-linux\"; })'"
    ).strip()
    print(f"Instantiated: {drv_path}")

    output = machine.succeed(
        f"nix-store --store unix:///run/harmonia-daemon/socket --realise {drv_path} 2>&1"
    )
    print(f"nix-store --realise output: {output}")

    # Read the built output
    out_path = output.strip().split("\n")[-1]
    built_output = machine.succeed(f"cat {out_path}")
    print(f"Builder output: {built_output}")
    assert "hello" in built_output, f"Expected 'hello' in output, got: {built_output}"

    print("All sandbox tests passed!")
  '';
}
