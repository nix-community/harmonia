# Platform-independent harmonia-gc options shared by the NixOS and
# nix-darwin modules. Scheduling (systemd dates vs launchd intervals)
# lives in the platform modules.
{ crane, nix-src }:
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.services.harmonia-dev;
  gcCfg = cfg.gc;
in
{
  options.services.harmonia-dev = {
    package = lib.mkOption {
      type = lib.types.package;
      default = (pkgs.callPackage ./packages.nix { inherit crane nix-src; }).harmonia;
      defaultText = lib.literalExpression "pkgs.harmonia";
      description = "The harmonia package";
    };

    gc = {
      enable = lib.mkEnableOption "harmonia-gc, a faster nix-collect-garbage";

      automatic = lib.mkOption {
        type = lib.types.bool;
        default = false;
        description = "Run garbage collection automatically on a schedule.";
      };

      deleteOlderThan = lib.mkOption {
        type = lib.types.nullOr lib.types.singleLineStr;
        default = null;
        example = "30d";
        description = "Delete profile generations older than this.";
      };

      ensureFree = lib.mkOption {
        type = lib.types.nullOr lib.types.singleLineStr;
        default = null;
        example = "50G";
        description = ''
          Free space until this much is available, then stop. Accepts an
          absolute size like "50G" or a percentage of the store's filesystem
          like "20%".
        '';
      };

      keepRecent = lib.mkOption {
        type = lib.types.nullOr lib.types.singleLineStr;
        default = null;
        example = "1d";
        description = ''
          Keep store paths registered within this time window. Avoids deleting
          build dependencies fetched during a recent build.
        '';
      };

      noVacuum = lib.mkOption {
        type = lib.types.bool;
        default = false;
        description = ''
          Skip the SQLite VACUUM after garbage collection. Enable on busy
          builders, where concurrent nix-daemon readers prevent cleanup of
          the database-sized WAL that VACUUM produces.
        '';
      };

      chunkSize = lib.mkOption {
        type = lib.types.nullOr lib.types.ints.positive;
        default = null;
        description = ''
          Number of dead paths invalidated per database transaction. Lower
          values keep the WAL (and its disk use) smaller during deletion at
          the cost of more checkpoints; null uses the built-in default.
        '';
      };

      gcRootsDirs = lib.mkOption {
        type = lib.types.listOf lib.types.str;
        default = [ ];
        example = [ "/mnt/extra-roots" ];
        description = ''
          Extra directories to scan for GC roots, treated like the standard
          gcroots directory. Nix only scans its own state directories; this
          keeps roots that live elsewhere from being collected.
        '';
      };

      extraArgs = lib.mkOption {
        type = lib.types.listOf lib.types.str;
        default = [ ];
        description = "Extra arguments to pass to harmonia-gc.";
      };

      argv = lib.mkOption {
        type = lib.types.listOf lib.types.str;
        internal = true;
        readOnly = true;
      };
    };
  };

  config.services.harmonia-dev.gc.argv = [
    "${cfg.package}/bin/harmonia-gc"
  ]
  ++ lib.optionals (gcCfg.deleteOlderThan != null) [
    "--delete-older-than"
    gcCfg.deleteOlderThan
  ]
  ++ lib.optionals (gcCfg.ensureFree != null) [
    "--ensure-free"
    gcCfg.ensureFree
  ]
  ++ lib.optionals (gcCfg.keepRecent != null) [
    "--keep-recent"
    gcCfg.keepRecent
  ]
  ++ lib.optional gcCfg.noVacuum "--no-vacuum"
  ++ lib.optionals (gcCfg.chunkSize != null) [
    "--chunk-size"
    (toString gcCfg.chunkSize)
  ]
  ++ lib.concatMap (d: [
    "--gc-roots-dir"
    d
  ]) gcCfg.gcRootsDirs
  ++ gcCfg.extraArgs;
}
