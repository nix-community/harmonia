{
  treefmt,
  rustfmt,
  nixfmt-rs,
  deadnix,
  taplo,
}:
treefmt.withConfig {
  runtimeInputs = [
    rustfmt
    nixfmt-rs
    deadnix
    taplo
  ];
  settings = {
    tree-root-file = "flake.lock";
    formatter = {
      rustfmt = {
        command = "rustfmt";
        options = [
          "--edition"
          "2024"
        ];
        includes = [ "*.rs" ];
      };
      nixfmt = {
        command = "nixfmt";
        includes = [ "*.nix" ];
      };
      deadnix = {
        command = "deadnix";
        options = [ "--edit" ];
        includes = [ "*.nix" ];
      };
      taplo = {
        command = "taplo";
        options = [ "format" ];
        includes = [ "*.toml" ];
      };
    };
  };
}
