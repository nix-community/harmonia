{ pkgs, self }:
pkgs.testers.runNixOSTest {
  name = "gc";

  nodes.machine =
    { pkgs, ... }:
    {
      imports = [ self.nixosModules.harmonia ];
      services.harmonia-dev.gc = {
        enable = true;
        keepRecent = "1h";
      };
      # The CA phases root a .drv and expect its output to survive, which
      # requires keep-outputs (mirrors nix-store --gc semantics).
      nix.settings.keep-outputs = true;
      virtualisation.writableStore = true;
      # nix from git for the BuildTraceV3 phase; referenced by store path in
      # the test script, so it must be in the VM's closure.
      virtualisation.additionalPaths = [ pkgs.nixVersions.git ];
      environment.systemPackages = [
        pkgs.hello
        pkgs.sqlite
      ];
    };

  testScript = ''
    machine.start()
    machine.wait_for_unit("multi-user.target")
    db = "/nix/var/nix/db/db.sqlite"

    def backdate(*paths: str) -> None:
        quoted = ",".join(f"'{p}'" for p in paths)
        machine.succeed(
            f"sqlite3 {db} \"UPDATE ValidPaths SET registrationTime = 1 WHERE path IN ({quoted})\""
        )

    def gc() -> None:
        machine.succeed("systemctl start harmonia-gc.service")

    # --- CA derivations: Realisations must keep drv<->output alive ---
    ca_drv_expr = (
        'derivation { '
        'name = "ca-test"; system = "${pkgs.stdenv.hostPlatform.system}"; '
        'builder = "/bin/sh"; args = ["-c" "echo hello > $out"]; '
        '__contentAddressed = true; outputHashMode = "recursive"; '
        'outputHashAlgo = "sha256"; }'
    )
    ca_out = machine.succeed(
        "nix-build"
        ' --option experimental-features "nix-command ca-derivations"'
        f" --no-out-link -E '{ca_drv_expr}'"
    ).strip()
    ca_drv = machine.succeed(
        f"sqlite3 {db} \"SELECT path FROM ValidPaths WHERE path LIKE '%ca-test.drv'\""
    ).strip()
    assert ca_drv, "drv not found in DB"
    backdate(ca_drv, ca_out)

    machine.succeed(f"nix-store --add-root /tmp/ca-out-root --indirect -r {ca_out}")
    gc()
    machine.succeed(f"test -e {ca_out}", f"test -e {ca_drv}")

    machine.succeed("rm /tmp/ca-out-root", f"ln -sf {ca_drv} /nix/var/nix/gcroots/ca-drv-root")
    gc()
    machine.succeed(f"test -e {ca_drv}", f"test -e {ca_out}")
    machine.succeed("rm -f /nix/var/nix/gcroots/ca-drv-root")

    # --- BuildTraceV3 (nix from git) ---
    machine.succeed(
        "cat > /tmp/ca-test2.nix <<'EOF'\n"
        "derivation {\n"
        '  name = "ca-test2";\n'
        '  system = "${pkgs.stdenv.hostPlatform.system}";\n'
        '  builder = "/bin/sh";\n'
        '  args = ["-c" "echo hello2 > $out"];\n'
        "  __contentAddressed = true;\n"
        '  outputHashMode = "recursive";\n'
        '  outputHashAlgo = "sha256";\n'
        "}\nEOF"
    )
    # --store local bypasses the system daemon, which would populate
    # Realisations instead of BuildTraceV3.
    ca_out2 = machine.succeed(
        "${pkgs.nixVersions.git}/bin/nix-build --store local"
        ' --option experimental-features "nix-command ca-derivations"'
        " --no-out-link /tmp/ca-test2.nix"
    ).strip()
    ca_drv2 = machine.succeed(
        f"sqlite3 {db} \"SELECT path FROM ValidPaths WHERE path LIKE '%ca-test2.drv'\""
    ).strip()
    assert ca_drv2, "ca-test2 drv not found in DB"
    ca_drv2_base = ca_drv2.split("/")[-1]
    bt_count = int(machine.succeed(
        f"sqlite3 {db} \"SELECT COUNT(*) FROM BuildTraceV3 WHERE drvPath = '{ca_drv2_base}'\""
    ).strip())
    assert bt_count > 0, f"BuildTraceV3 has no entry for {ca_drv2_base}"
    backdate(ca_drv2, ca_out2)

    machine.succeed(f"ln -sf {ca_out2} /nix/var/nix/gcroots/ca-out2-root")
    gc()
    machine.succeed(f"test -e {ca_out2}", f"test -e {ca_drv2}")

    machine.succeed(
        "rm /nix/var/nix/gcroots/ca-out2-root",
        f"ln -sf {ca_drv2} /nix/var/nix/gcroots/ca-drv2-root",
    )
    gc()
    machine.succeed(f"test -e {ca_drv2}", f"test -e {ca_out2}")
    machine.succeed("rm -f /nix/var/nix/gcroots/ca-drv2-root")

    # --- dead path is collected, profile-pinned path survives ---
    machine.succeed("echo gc-victim > /tmp/gc-dead")
    dead = machine.succeed("nix-store --add /tmp/gc-dead").strip()
    backdate(dead)
    machine.succeed(f"test -e {dead}")
    gc()
    machine.fail(f"test -e {dead}")
    machine.succeed("hello --version")

    # --- keepRecent pins freshly registered paths ---
    machine.succeed("echo recent > /tmp/gc-recent")
    recent = machine.succeed("nix-store --add /tmp/gc-recent").strip()
    gc()
    machine.succeed(f"test -e {recent}")
  '';
}
