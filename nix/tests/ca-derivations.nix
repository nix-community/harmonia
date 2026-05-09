{ pkgs, self }:
let
  # The build-trace-v2 endpoint only exists in unreleased Nix; pin both nodes
  # to nixVersions.git so the client understands the new format.
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

        services.harmonia-dev.cache.enable = true;
        services.harmonia-dev.cache.signKeyPaths = [ "${../../tests/cache.sk}" ];

        networking.firewall.allowedTCPPorts = [ 5000 ];
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
    harmonia.wait_for_unit("harmonia-dev.socket")
    harmonia.wait_for_open_port(5000)
    client01.wait_until_succeeds("timeout 1 curl -f http://harmonia:5000/nix-cache-info")

    # Build the CA derivation on the server so its realisation lands in the
    # on-disk BuildTraceV3 table that harmonia-cache reads.
    drv = harmonia.succeed(
        "nix-instantiate --extra-experimental-features ca-derivations ${caDrv}"
    ).strip()
    drv_base = drv.removeprefix("/nix/store/")
    out = harmonia.succeed(f"nix-store --realise {drv}").strip()
    print(f"CA drv {drv} -> {out}")

    # Harmonia must serve the build trace at the v2 endpoint without a
    # restart, i.e. it must observe BuildTraceV3 rows written after it opened
    # the database read-only.
    body = harmonia.wait_until_succeeds(
        f"curl -sf http://localhost:5000/build-trace-v2/{drv_base}/out.doi"
    )
    print(body)
    data = json.loads(body)
    assert data["outPath"] == out.removeprefix("/nix/store/"), data
    assert "signatures" in data, data

    # The real test: the upstream Nix client must be able to substitute
    # the CA derivation via harmonia. With -j0 it cannot build locally,
    # so this only succeeds if the realisation lookup over HTTP works.
    client01.succeed("nix-store --delete " + out + " || true")
    client01.succeed(
        f"nix-store --realise {drv} -j0 "
        "--option substituters http://harmonia:5000 "
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
