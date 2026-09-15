{ crane, nix-src }:
{
  config,
  pkgs,
  lib,
  ...
}:
let
  cfg = config.services.harmonia-dev;
  cacheCfg = cfg.cache;
  daemonCfg = cfg.daemon;
  gcCfg = cfg.gc;

  format = pkgs.formats.toml { };
  configFile = format.generate "harmonia.toml" cacheCfg.settings;

  signKeyPaths =
    cacheCfg.signKeyPaths ++ (if cacheCfg.signKeyPath != null then [ cacheCfg.signKeyPath ] else [ ]);
  credentials = lib.imap0 (i: signKeyPath: {
    id = "sign-key-${toString i}";
    path = signKeyPath;
  }) signKeyPaths;
in
{
  imports = [
    (lib.modules.importApply ./gc-options.nix { inherit crane nix-src; })
    # Renamed options for flat harmonia-dev -> harmonia-dev.cache
    (lib.mkRenamedOptionModule
      [ "services" "harmonia-dev" "enable" ]
      [ "services" "harmonia-dev" "cache" "enable" ]
    )
    (lib.mkRenamedOptionModule
      [ "services" "harmonia-dev" "signKeyPath" ]
      [ "services" "harmonia-dev" "cache" "signKeyPath" ]
    )
    (lib.mkRenamedOptionModule
      [ "services" "harmonia-dev" "signKeyPaths" ]
      [ "services" "harmonia-dev" "cache" "signKeyPaths" ]
    )
    (lib.mkRenamedOptionModule
      [ "services" "harmonia-dev" "settings" ]
      [ "services" "harmonia-dev" "cache" "settings" ]
    )
    # Note: package stays at the top level
  ];

  options = {
    services.harmonia-dev = {
      cache = {
        enable = lib.mkEnableOption "Harmonia: Nix binary cache written in Rust";

        signKeyPath = lib.mkOption {
          type = lib.types.nullOr lib.types.path;
          default = null;
          description = "DEPRECATED: Use `services.harmonia-dev.cache.signKeyPaths` instead. Path to the signing key to use for signing the cache";
        };

        signKeyPaths = lib.mkOption {
          type = lib.types.listOf lib.types.path;
          default = [ ];
          description = "Paths to the signing keys to use for signing the cache";
        };

        settings = lib.mkOption {
          type = lib.types.submodule { freeformType = format.type; };

          description = "Settings to merge with the default configuration";
        };
      };

      gc = {
        dates = lib.mkOption {
          type = with lib.types; either singleLineStr (listOf str);
          apply = lib.toList;
          default = [ "03:15" ];
          example = "weekly";
          description = ''
            When to run garbage collection. Calendar event in the format
            specified by {manpage}`systemd.time(7)`.
          '';
        };

        randomizedDelaySec = lib.mkOption {
          type = lib.types.singleLineStr;
          default = "0";
          example = "45min";
          description = "Randomized delay before each run.";
        };

        persistent = lib.mkOption {
          type = lib.types.bool;
          default = true;
          description = "Run on next boot if a scheduled run was missed.";
        };
      };

      daemon = {
        enable = lib.mkEnableOption "Harmonia daemon: Nix daemon protocol implementation";

        socketPath = lib.mkOption {
          type = lib.types.str;
          default = "/run/harmonia-daemon/socket";
          description = "Path where the daemon socket will be created";
        };

        storeDir = lib.mkOption {
          type = lib.types.str;
          default = "/nix/store";
          description = "Path to the Nix store directory";
        };

        dbPath = lib.mkOption {
          type = lib.types.str;
          default = "/nix/var/nix/db/db.sqlite";
          description = "Path to the Nix database";
        };

        logLevel = lib.mkOption {
          type = lib.types.str;
          default = "info";
          description = "Log level for the daemon";
        };
      };
    };
  };

  config = lib.mkMerge [
    (lib.mkIf cacheCfg.enable {
      warnings =
        if cacheCfg.signKeyPath != null then
          [
            "`services.harmonia-dev.cache.signKeyPath` is deprecated, use `services.harmonia-dev.cache.signKeyPaths` instead"
          ]
        else
          [ ];

      services.harmonia-dev.cache.settings = builtins.mapAttrs (_: v: lib.mkDefault v) {
        bind = "[::]:5000";
        workers = 4;
        max_connection_rate = 256;
        priority = 50;
      };

      # Socket activation lets the service run with PrivateNetwork; the
      # inherited fd keeps referring to the host netns.
      systemd.sockets.harmonia-dev = {
        description = "harmonia binary cache socket";
        wantedBy = [ "sockets.target" ];
        socketConfig.ListenStream =
          let
            b = cacheCfg.settings.bind;
          in
          if lib.hasPrefix "unix:" b then lib.removePrefix "//" (lib.removePrefix "unix:" b) else b;
      };

      systemd.services.harmonia-dev = {
        description = "harmonia binary cache service";

        requires = [ "harmonia-dev.socket" ];
        after = [ "harmonia-dev.socket" ];

        environment = {
          CONFIG_FILE = lib.mkIf (configFile != null) configFile;
          SIGN_KEY_PATHS = lib.strings.concatMapStringsSep " " (
            credential: "%d/${credential.id}"
          ) credentials;
          # print stack traces
          RUST_LOG = "info,actix_web=debug";
          RUST_BACKTRACE = "1";
        };

        serviceConfig = {
          Type = "notify";
          WatchdogSec = 15;
          Restart = "on-failure";
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

          # accept(2) on the inherited fd is exempt from both restrictions.
          PrivateNetwork = true;
          RestrictAddressFamilies = [ "AF_UNIX" ];
          IPAddressDeny = "any";

          PrivateTmp = true;
          PrivateDevices = true;
          PrivateMounts = true;
          NoNewPrivileges = true;
          ProtectSystem = "strict";
          ProtectHome = true;
          LockPersonality = true;

          LimitNOFILE = 65536;
        };
      };
    })

    (lib.mkIf gcCfg.enable {
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

      systemd.services.harmonia-gc = {
        description = "Harmonia Nix Garbage Collector";
        # `nix config show` for keep-derivations/keep-outputs.
        path = [ config.nix.package ];
        serviceConfig = {
          Type = "oneshot";
          ExecStart = lib.escapeShellArgs gcCfg.argv;
        };
        startAt = lib.optionals gcCfg.automatic gcCfg.dates;
        restartIfChanged = false;
      };

      systemd.timers.harmonia-gc = lib.mkIf gcCfg.automatic {
        timerConfig = {
          RandomizedDelaySec = gcCfg.randomizedDelaySec;
          Persistent = gcCfg.persistent;
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
            ExecStart = "${cfg.package}/bin/harmonia-daemon";
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
              (dirOf daemonCfg.dbPath) # Need write access for WAL and SHM files
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
