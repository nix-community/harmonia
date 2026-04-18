{
  projectRootFile = "flake.lock";
  programs.rustfmt = {
    enable = true;
    edition = "2024";
  };
  programs.nixfmt.enable = true;
  programs.deadnix.enable = true;
  programs.clang-format.enable = true;
  programs.taplo.enable = true;
}
