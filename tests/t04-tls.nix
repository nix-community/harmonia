(import ./lib.nix) {
  name = "t04-tls";

  nodes.harmonia = {
    imports = [ ../module.nix ];

    services.harmonia-dev.enable = true;
    services.harmonia-dev.settings.tls_cert_path = ./tls-cert.pem;
    services.harmonia-dev.settings.tls_key_path = ./tls-key.pem;
  };

  testScript = ''
    harmonia.wait_until_succeeds("curl --cacert ${./tls-cert.pem} https://localhost:5000/version")
  '';
}
