{
  config,
  pkgs,
  lib,
  ...
}:
let
  cfg = config.services.harmonia-dev;
  daemonCfg = config.services.harmonia-daemon;

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

    services.harmonia-daemon = {
      enable = lib.mkEnableOption (lib.mdDoc "Harmonia daemon: Nix daemon protocol implementation");

      socketPath = lib.mkOption {
        type = lib.types.str;
        default = "/run/harmonia-daemon/socket";
        description = lib.mdDoc "Path where the daemon socket will be created";
      };

      storeDir = lib.mkOption {
        type = lib.types.str;
        default = "/nix/store";
        description = lib.mdDoc "Path to the Nix store directory";
      };

      dbPath = lib.mkOption {
        type = lib.types.str;
        default = "/nix/var/nix/db/db.sqlite";
        description = lib.mdDoc "Path to the Nix database";
      };

      logLevel = lib.mkOption {
        type = lib.types.str;
        default = "info";
        description = lib.mdDoc "Log level for the daemon";
      };

      package = lib.mkOption {
        type = lib.types.path;
        default = pkgs.callPackage ./. { };
        description = "The harmonia package containing harmonia-daemon";
      };
    };
  };

  config = lib.mkMerge [
    (lib.mkIf cfg.enable {
      warnings =
        if cfg.signKeyPath != null then
          [
            ''`services.harmonia-dev.signKeyPath` is deprecated, use `services.harmonia-dev.signKeyPaths` instead''
          ]
        else
          [ ];

      services.harmonia-dev.settings = builtins.mapAttrs (_: v: lib.mkDefault v) (
        {
          bind = "[::]:5000";
          workers = 4;
          max_connection_rate = 256;
          priority = 50;
        }
        // lib.optionalAttrs daemonCfg.enable {
          daemon_socket = daemonCfg.socketPath;
        }
      );

      systemd.services.harmonia-dev = {
        description = "harmonia binary cache service";

        requires = if daemonCfg.enable then [ "harmonia-daemon.service" ] else [ "nix-daemon.socket" ];
        after = [ "network.target" ] ++ lib.optional daemonCfg.enable "harmonia-daemon.service";
        wantedBy = [ "multi-user.target" ];

        environment = {
          NIX_REMOTE = "daemon";
          LIBEV_FLAGS = "4"; # go ahead and mandate epoll(2)
          CONFIG_FILE = lib.mkIf (configFile != null) configFile;
          SIGN_KEY_PATHS = lib.strings.concatMapStringsSep " " (
            credential: "%d/${credential.id}"
          ) credentials;
          # print stack traces
          RUST_LOG = "actix_web=debug";
          RUST_BACKTRACE = "1";
        };

        # Note: it's important to set this for nix-store, because it wants to use
        # $HOME in order to use a temporary cache dir. bizarre failures will occur
        # otherwise
        environment.HOME = "/run/harmonia";

        serviceConfig = {
          ExecStart = "${cfg.package}/bin/harmonia-cache";

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
    })

    (lib.mkIf daemonCfg.enable {
      systemd.services.harmonia-daemon =
        let
          daemonConfig = {
            socket_path = daemonCfg.socketPath;
            store_dir = daemonCfg.storeDir;
            db_path = daemonCfg.dbPath;
            log_level = daemonCfg.logLevel;
          };
          daemonConfigFile = format.generate "harmonia-daemon.toml" daemonConfig;
        in
        {
          description = "Harmonia Nix daemon protocol server";
          after = [ "network.target" ];
          wantedBy = [ "multi-user.target" ];

          environment = {
            RUST_LOG = daemonCfg.logLevel;
            RUST_BACKTRACE = "1";
            HARMONIA_DAEMON_CONFIG = daemonConfigFile;
          };

          serviceConfig = {
            Type = "simple";
            ExecStart = "${daemonCfg.package}/bin/harmonia-daemon";
            Restart = "on-failure";
            RestartSec = 5;

            # Socket will be created at runtime
            RuntimeDirectory = "harmonia-daemon";

            # Run as root to access the Nix database
            # Note: The Nix database is owned by root and requires root access
            NoNewPrivileges = true;
            PrivateTmp = true;
            ProtectSystem = "strict";
            ProtectHome = true;
            # SQLite needs write access for WAL mode
            ReadWritePaths = [
              (builtins.dirOf daemonCfg.dbPath) # Need write access for WAL and SHM files
            ];
            ReadOnlyPaths = [
              daemonCfg.storeDir
            ];

            # System call filtering
            SystemCallFilter = [
              "@system-service"
              "~@privileged"
              "@chown" # for sockets
              "~@resources"
            ];
            SystemCallArchitectures = "native";

            # Capabilities
            CapabilityBoundingSet = "";

            # Device access
            DeviceAllow = [ "" ];
            PrivateDevices = true;

            # Kernel protection
            ProtectKernelModules = true;
            ProtectKernelTunables = true;
            ProtectControlGroups = true;
            ProtectKernelLogs = true;
            ProtectHostname = true;
            ProtectClock = true;

            # Memory protection
            MemoryDenyWriteExecute = true;
            LockPersonality = true;

            # Process visibility
            ProcSubset = "pid";
            ProtectProc = "invisible";

            # Namespace restrictions
            RestrictNamespaces = true;
            PrivateMounts = true;

            # Network restrictions
            RestrictAddressFamilies = "AF_UNIX";
            PrivateNetwork = false;

            # Resource limits
            LimitNOFILE = 65536;
            RestrictRealtime = true;

            # Misc restrictions
            UMask = "0077";
          };
        };
    })
  ];
}
