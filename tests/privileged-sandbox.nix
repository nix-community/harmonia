{ pkgs, self }:
pkgs.testers.nixosTest {
  name = "privileged-sandbox";
  globalTimeout = 120;
  nodes.machine = {
    imports = [ self.outputs.nixosModules.harmonia ];

    services.harmonia-dev.daemon.enable = true;

    # Disable the regular nix-daemon to avoid conflicts
    systemd.services.nix-daemon.enable = false;
    systemd.sockets.nix-daemon.enable = false;

    # Set up nixbld group with build users.
    # nixbld1 gets an extra group (testgrp) to verify that supplementary
    # GIDs are resolved via getgrouplist() — the plumbing that
    # acquire_simple_user_lock + LinuxSandbox Group mode relies on.
    users.groups.nixbld = { };
    users.groups.testgrp.gid = 1234;
    users.users = builtins.listToAttrs (
      builtins.genList (
        i:
        let
          n = i + 1;
        in
        {
          name = "nixbld${toString n}";
          value = {
            isSystemUser = true;
            group = "nixbld";
            extraGroups = if n == 1 then [ "testgrp" ] else [ ];
          };
        }
      ) 4
    );

    system.extraDependencies = [ pkgs.hello ];
  };

  testScript = ''
    machine.wait_for_unit("multi-user.target")
    machine.wait_for_unit("harmonia-daemon.service")
    machine.wait_for_file("/run/harmonia-daemon/socket")

    # Verify build users exist with correct supplementary groups
    output = machine.succeed("id nixbld1")
    print(f"nixbld1: {output}")
    assert "testgrp" in output, f"nixbld1 should have testgrp supplementary group: {output}"

    output = machine.succeed("id nixbld2")
    print(f"nixbld2: {output}")
    assert "testgrp" not in output, f"nixbld2 should not have testgrp: {output}"

    # Verify getgrouplist(3) returns the supplementary group for nixbld1.
    # This is the same call harmonia's acquire_simple_user_lock uses.
    machine.succeed("python3 -c \"\nimport ctypes, ctypes.util, struct\nlib = ctypes.CDLL(ctypes.util.find_library('c'))\nMAX = 32\ngids = (ctypes.c_uint * MAX)()\nn = ctypes.c_int(MAX)\nassert lib.getgrouplist(b'nixbld1', 0, gids, ctypes.byref(n)) >= 0\nresult = [gids[i] for i in range(n.value)]\nprint(f'getgrouplist(nixbld1): {result}')\nassert 1234 in result, f'expected gid 1234 (testgrp) in {result}'\n\"")

    # Verify harmonia-daemon is healthy and serving on its socket
    machine.succeed("systemctl is-active harmonia-daemon.service")

    # Verify harmonia-daemon can handle a nix-store query through the socket
    machine.succeed("nix-store --store unix:///run/harmonia-daemon/socket -q --hash ${pkgs.hello}")

    print("All privileged sandbox plumbing tests passed!")
  '';
}
