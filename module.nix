{ config, pkgs, lib, ... }:
let
  cfg = config.services.harmonia-dev;

  format = pkgs.formats.toml { };
  configFile = format.generate "harmonia.toml" cfg.settings;
in
{
  options = {
    services.harmonia-dev = {
      enable = lib.mkEnableOption (lib.mdDoc "Harmonia: Nix binary cache written in Rust");

      signKeyPath = lib.mkOption {
        type = lib.types.nullOr lib.types.path;
        default = null;
        description = lib.mdDoc "Path to the signing key to use for signing the cache";
      };

      settings = lib.mkOption {
        type = lib.types.submodule {
          freeformType = format.type;
        };

        description = lib.mdDoc "Settings to merge with the default configuration";
      };

      package = lib.mkOption {
        type = lib.types.path;
        default = pkgs.callPackage ./. { };
        description = "The harmonia package";
      };
    };
  };

  config = lib.mkIf cfg.enable {
    services.harmonia-dev.settings = builtins.mapAttrs (_: v: lib.mkDefault v) {
      bind = "[::]:5000";
      workers = 4;
      max_connection_rate = 256;
      priority = 50;
    };

    systemd.services.harmonia-dev = {
      description = "harmonia binary cache service";

      requires = [ "nix-daemon.socket" ];
      after = [ "network.target" ];
      wantedBy = [ "multi-user.target" ];

      environment = {
        NIX_REMOTE = "daemon";
        LIBEV_FLAGS = "4"; # go ahead and mandate epoll(2)
        CONFIG_FILE = lib.mkIf (configFile != null) configFile;
        SIGN_KEY_PATH = lib.mkIf (cfg.signKeyPath != null) "%d/sign-key";
        RUST_LOG = "info";
      };

      # Note: it's important to set this for nix-store, because it wants to use
      # $HOME in order to use a temporary cache dir. bizarre failures will occur
      # otherwise
      environment.HOME = "/run/harmonia";


      serviceConfig = {
        ExecStart = "${cfg.package}/bin/harmonia";

        User = "harmonia";
        Group = "harmonia";
        DynamicUser = true;
        PrivateUsers = true;
        DeviceAllow = [ "" ];
        UMask = "0066";

        RuntimeDirectory = "harmonia";
        LoadCredential = lib.optional (cfg.signKeyPath != null) "sign-key:${cfg.signKeyPath}";

        SystemCallFilter = [
          "@system-service"
          "~@privileged"
          "~@resources"
        ];
        CapabilityBoundingSet = "";
        ProtectKernelModules = true;
        ProtectKernelTunables = true;
        ProtectControlGroups = true;
        ProtectKernelLogs = true;
        ProtectHostname = true;
        ProtectClock = true;
        RestrictRealtime = true;
        MemoryDenyWriteExecute = true;
        ProcSubset = "pid";
        ProtectProc = "invisible";
        RestrictNamespaces = true;
        SystemCallArchitectures = "native";

        PrivateNetwork = false;
        PrivateTmp = true;
        PrivateDevices = true;
        PrivateMounts = true;
        NoNewPrivileges = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        LockPersonality = true;
        RestrictAddressFamilies = "AF_UNIX AF_INET AF_INET6";

        LimitNOFILE = 65536;
      };
    };
  };
}
