(import ./lib.nix) (
  { pkgs, ... }:
  let
    testServe = pkgs.runCommand "test" { } ''
      mkdir -p $out/dir
      echo file > $out/dir/file
      ln -s /etc/passwd $out/forbidden-symlink
      ln -s $out/dir/file $out/allowed-symlink

      # check unicode
      echo test > "$out/🦄"
      # check invalid utf-8
      touch $(echo -e "test\x80")
    '';
  in
  {
    name = "t00-simple";

    nodes = {
      harmonia =
        { pkgs, ... }:
        {
          imports = [ ../module.nix ];

          services.harmonia-dev.cache.enable = true;

          networking.firewall.allowedTCPPorts = [ 5000 ];
          system.extraDependencies = [
            pkgs.hello
            testServe
          ];
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

    testScript =
      let
        hashPart = pkg: builtins.substring (builtins.stringLength builtins.storeDir + 1) 32 pkg.outPath;
      in
      ''
        import json
        start_all()

        client01.wait_until_succeeds("timeout 1 curl -f http://harmonia:5000")
        client01.succeed("curl -f http://harmonia:5000/nix-cache-info")

        client01.wait_until_succeeds("nix copy --from http://harmonia:5000/ ${pkgs.hello}")
        out = client01.wait_until_succeeds("curl http://harmonia:5000/${hashPart pkgs.hello}.ls")
        data = json.loads(out)
        print(out)
        assert data["version"] == 1, "version is not correct"
        assert data["root"]["entries"]["bin"]["type"] == "directory", "expect bin directory in listing"
        client01.succeed("${pkgs.hello}/bin/hello")

        # test unicode
        client01.succeed("nix copy --from http://harmonia:5000/ ${testServe}")

        print("download ${testServe}")
        out = client01.wait_until_succeeds("curl -v http://harmonia:5000/serve/${hashPart testServe}/")
        print(out)
        assert "dir/" in out, "dir/ not in listing"

        out = client01.wait_until_succeeds("curl -v http://harmonia:5000/serve/${hashPart testServe}/dir")
        print(out)
        assert "file" in out, "file not in listing"

        out = client01.wait_until_succeeds("curl -v http://harmonia:5000/serve/${hashPart testServe}/dir/file").strip()
        print(out)
        assert "file" == out, f"expected 'file', got '{out}'"

        out = client01.wait_until_succeeds("curl -v http://harmonia:5000/serve/${hashPart testServe}/🦄").strip()
        print(out)
        assert "test" == out, f"expected 'test', got '{out}'"

        # TODO: this is still broken
        #out = client01.wait_until_succeeds("curl -v http://harmonia:5000/serve/${hashPart testServe}/test\\x80").strip()
        #print(out)
        #assert "test" == out, f"expected 'test', got '{out}'"
      '';
  }
)
