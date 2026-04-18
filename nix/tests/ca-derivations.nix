{ pkgs, self }:
let
  # The build-trace-v2 endpoint and the `realisation-with-path-not-hash`
  # worker-protocol feature only exist in unreleased Nix; pin both nodes to
  # nixVersions.git so the daemon harmonia talks to and the client both
  # understand the new format.
  nixGit = pkgs.nixVersions.git;

  caDrv = pkgs.writeText "ca.nix" ''
    derivation {
      name = "ca-test";
      system = builtins.currentSystem;
      builder = "/bin/sh";
      args = [ "-c" "echo hello-from-ca > $out" ];
      __contentAddressed = true;
      outputHashMode = "recursive";
      outputHashAlgo = "sha256";
    }
  '';
in
pkgs.testers.nixosTest {
  name = "ca-derivations";

  nodes = {
    harmonia =
      { ... }:
      {
        imports = [ self.nixosModules.harmonia ];

        nix.package = nixGit;
        nix.settings.experimental-features = [
          "nix-command"
          "ca-derivations"
        ];
        nix.settings.sandbox = false;

        # First instance: harmonia-cache talking directly to nix-daemon.
        services.harmonia-dev.cache.enable = true;
        services.harmonia-dev.cache.signKeyPaths = [ "${../../tests/cache.sk}" ];

        # Second instance: harmonia-cache backed by harmonia-daemon reading
        # the SQLite DB directly, so we exercise harmonia-daemon's own
        # QueryRealisation implementation.
        services.harmonia-dev.daemon.enable = true;
        systemd.services.harmonia-dev-via-daemon =
          let
            cfg = (pkgs.formats.toml { }).generate "harmonia.toml" {
              bind = "[::]:5001";
              daemon_socket = "/run/harmonia-daemon/socket";
              priority = 30;
            };
          in
          {
            wantedBy = [ "multi-user.target" ];
            after = [ "harmonia-daemon.service" ];
            requires = [ "harmonia-daemon.service" ];
            environment.CONFIG_FILE = "${cfg}";
            serviceConfig.DynamicUser = true;
            serviceConfig.ExecStart = "${
              self.packages.${pkgs.stdenv.hostPlatform.system}.harmonia
            }/bin/harmonia-cache";
          };

        networking.firewall.allowedTCPPorts = [
          5000
          5001
        ];
      };

    client01 =
      { lib, ... }:
      {
        nix.package = nixGit;
        nix.settings.substituters = lib.mkForce [ "http://harmonia:5000" ];
        nix.settings.trusted-public-keys = [ (builtins.readFile ../../tests/cache.pk) ];
        nix.settings.experimental-features = [
          "nix-command"
          "ca-derivations"
        ];
        nix.settings.sandbox = false;
      };
  };

  testScript = ''
    import json

    start_all()
    harmonia.wait_for_unit("harmonia-dev.service")
    harmonia.wait_for_open_port(5000)
    client01.wait_until_succeeds("timeout 1 curl -f http://harmonia:5000/nix-cache-info")

    # Build the CA derivation on the server so its realisation lands in both
    # nix-daemon and the on-disk BuildTraceV3 table that harmonia-daemon reads.
    drv = harmonia.succeed(
        "nix-instantiate --extra-experimental-features ca-derivations ${caDrv}"
    ).strip()
    drv_base = drv.removeprefix("/nix/store/")
    out = harmonia.succeed(f"nix-store --realise {drv}").strip()
    print(f"CA drv {drv} -> {out}")

    # harmonia-daemon opened the DB read-only before BuildTraceV3 existed; restart
    # so it picks up the CA schema created by the build above.
    harmonia.succeed("systemctl restart harmonia-daemon.service harmonia-dev-via-daemon.service")
    harmonia.wait_for_open_port(5001)

    for port in (5000, 5001):
        # Harmonia must serve the build trace at the v2 endpoint, both when
        # backed by nix-daemon (5000) and by harmonia-daemon (5001).
        body = harmonia.succeed(
            f"curl -sf http://localhost:{port}/build-trace-v2/{drv_base}/out.doi"
        )
        print(f"port {port}: {body}")
        data = json.loads(body)
        assert data["outPath"] == out.removeprefix("/nix/store/"), (port, data)
        assert "signatures" in data, (port, data)

        # The real test: the upstream Nix client must be able to substitute
        # the CA derivation via harmonia. With -j0 it cannot build locally,
        # so this only succeeds if the realisation lookup over HTTP works.
        client01.succeed("nix-store --delete " + out + " || true")
        client01.succeed(
            f"nix-store --realise {drv} -j0 "
            f"--option substituters http://harmonia:{port} "
            "--option require-sigs false "
            "--extra-experimental-features ca-derivations"
        )
        client01.succeed(f"grep -q hello-from-ca {out}")

    # Verify the realisation was registered on the client side.
    client01.succeed(
        "nix --extra-experimental-features 'nix-command ca-derivations' "
        f"realisation info {drv}^out"
    )
  '';
}
