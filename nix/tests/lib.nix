test:
{
  pkgs ? import <nixpkgs> { },
  ...
}@args:
(pkgs.testers.runNixOSTest {
  # speed-up evaluation
  defaults.documentation.enable = pkgs.lib.mkDefault false;
  # Faster dhcp
  defaults.networking.useNetworkd = pkgs.lib.mkDefault true;
  # to accept external dependencies such as disko
  node.specialArgs.inputs = args;
  imports = [ test ];
}).config.result
