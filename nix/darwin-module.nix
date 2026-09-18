{ crane, nix-src }:
{
  config,
  lib,
  ...
}:
let
  gcCfg = config.services.harmonia-dev.gc;
in
{
  imports = [ (lib.modules.importApply ./gc-options.nix { inherit crane nix-src; }) ];

  options.services.harmonia-dev.gc.startCalendarInterval = lib.mkOption {
    type = with lib.types; listOf (attrsOf int);
    default = [
      {
        Hour = 3;
        Minute = 15;
      }
    ];
    description = ''
      When to run garbage collection, as launchd
      {manpage}`launchd.plist(5)` StartCalendarInterval entries.
    '';
  };

  config = lib.mkIf gcCfg.enable {
    assertions = [
      {
        assertion = gcCfg.automatic -> config.nix.enable;
        message = "services.harmonia-dev.gc.automatic requires nix.enable";
      }
    ];

    warnings = lib.optional (gcCfg.automatic && config.nix.gc.automatic) ''
      Both services.harmonia-dev.gc.automatic and nix.gc.automatic are enabled.
      Disable nix.gc.automatic to avoid running two garbage collectors.
    '';

    launchd.daemons.harmonia-gc.serviceConfig = {
      ProgramArguments = gcCfg.argv;
      RunAtLoad = false;
      StartCalendarInterval = lib.mkIf gcCfg.automatic gcCfg.startCalendarInterval;
      # `nix config show` for keep-derivations/keep-outputs. Raw
      # ProgramArguments bypass nix-darwin's `path` wrapper, so put nix on
      # PATH explicitly. When nix-darwin does not manage nix (external
      # installer), config.nix.package is unavailable, so fall back to the
      # standard profile/installer locations.
      EnvironmentVariables.PATH =
        lib.optionalString config.nix.enable "${config.nix.package}/bin:"
        + "/nix/var/nix/profiles/default/bin:/usr/local/bin:/usr/bin:/bin";
    };
  };
}
