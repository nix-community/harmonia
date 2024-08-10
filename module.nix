{
  config,
  pkgs,
  lib,
  ...
}:
let
  cfg = config.services.harmonia-dev;

  format = pkgs.formats.toml { };
  configFile = format.generate "harmonia.toml" cfg.settings;

  signKeyPaths = cfg.signKeyPaths ++ (if cfg.signKeyPath != null then [ cfg.signKeyPath ] else [ ]);
  credentials = lib.imap0 (i: signKeyPath: {
    id = "sign-key-${builtins.toString i}";
    path = signKeyPath;
  }) signKeyPaths;
in
{
  options = {
    services.harmonia-dev = {
      enable = lib.mkEnableOption (lib.mdDoc "Harmonia: Nix binary cache written in Rust");

      signKeyPath = lib.mkOption {
        type = lib.types.nullOr lib.types.path;
        default = null;
        description = lib.mdDoc "DEPRECATED: Use `services.harmonia-dev.signKeyPaths` instead. Path to the signing key to use for signing the cache";
      };

      signKeyPaths = lib.mkOption {
        type = lib.types.listOf lib.types.path;
        default = [ ];
        description = lib.mdDoc "Paths to the signing keys to use for signing the cache";
      };

      settings = lib.mkOption {
        type = lib.types.submodule { freeformType = format.type; };

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
    warnings =
      if cfg.signKeyPath != null then
        [
          ''`services.harmonia-dev.signKeyPath` is deprecated, use `services.harmonia-dev.signKeyPaths` instead''
        ]
      else
        [ ];

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
        SIGN_KEY_PATHS = lib.strings.concatMapStringsSep " " (
          credential: "%d/${credential.id}"
        ) credentials;
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
        LoadCredential = builtins.map (credential: "${credential.id}:${credential.path}") credentials;

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
