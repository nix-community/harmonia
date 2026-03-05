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

  format = pkgs.formats.toml { };
  configFile = format.generate "harmonia.toml" cacheCfg.settings;

  signKeyPaths =
    cacheCfg.signKeyPaths ++ (if cacheCfg.signKeyPath != null then [ cacheCfg.signKeyPath ] else [ ]);
  credentials = lib.imap0 (i: signKeyPath: {
    id = "sign-key-${builtins.toString i}";
    path = signKeyPath;
  }) signKeyPaths;
in
{
  imports = [
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
      package = lib.mkOption {
        type = lib.types.package;
        default = (pkgs.callPackage ./packages.nix { inherit crane nix-src; }).harmonia;
        defaultText = lib.literalExpression "pkgs.harmonia";
        description = "The harmonia package";
      };

      cache = {
        enable = lib.mkEnableOption ("Harmonia: Nix binary cache written in Rust");

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

      daemon = {
        enable = lib.mkEnableOption ("Harmonia daemon: Nix daemon protocol implementation");

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

        sandbox = lib.mkOption {
          type = lib.types.bool;
          default = false;
          description = ''
            Enable build sandbox.  On Linux this uses user namespaces;
            when running as root, build UIDs are auto-allocated via
            file locks.  On macOS this requires `buildUsersGroup`.
          '';
        };

        buildUsersGroup = lib.mkOption {
          type = lib.types.nullOr lib.types.str;
          default = null;
          description = ''
            Unix group whose members serve as build users.  Only used
            on macOS.  On Linux, UIDs are auto-allocated and this is
            ignored.
          '';
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

      services.harmonia-dev.cache.settings = builtins.mapAttrs (_: v: lib.mkDefault v) (
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
          daemonConfig =
            {
              socket_path = daemonCfg.socketPath;
              store_dir = daemonCfg.storeDir;
              db_path = daemonCfg.dbPath;
              log_level = daemonCfg.logLevel;
              sandbox = daemonCfg.sandbox;
            }
            // lib.optionalAttrs (daemonCfg.buildUsersGroup != null) {
              build_users_group = daemonCfg.buildUsersGroup;
            };
          daemonConfigFile = format.generate "harmonia-daemon.toml" daemonConfig;
          usesSandbox = daemonCfg.sandbox;
          poolDir = "/nix/var/nix/userpool";
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

          serviceConfig =
            if usesSandbox then
              {
                # Sandbox enabled: the daemon uses user namespaces and
                # (when root) auto-allocated build UIDs.  Most systemd
                # sandboxing is incompatible because the builder child
                # processes need to create user/mount namespaces.
                Type = "simple";
                ExecStart = "${cfg.package}/bin/harmonia-daemon";
                Restart = "on-failure";
                RestartSec = 5;
                RuntimeDirectory = "harmonia-daemon";
                LimitNOFILE = 65536;
              }
            else
              {
                # No build users: daemon only serves store queries, so
                # we can lock it down tightly.
                Type = "simple";
                ExecStart = "${cfg.package}/bin/harmonia-daemon";
                Restart = "on-failure";
                RestartSec = 5;
                RuntimeDirectory = "harmonia-daemon";

                NoNewPrivileges = true;
                CapabilityBoundingSet = "";

                PrivateTmp = true;
                ProtectSystem = "strict";
                ProtectHome = true;
                ReadWritePaths = [
                  (builtins.dirOf daemonCfg.dbPath)
                ];
                ReadOnlyPaths = [
                  daemonCfg.storeDir
                ];

                SystemCallFilter = [
                  "@system-service"
                  "@chown"
                ];
                SystemCallArchitectures = "native";

                DeviceAllow = [ "" ];
                PrivateDevices = true;

                ProtectKernelModules = true;
                ProtectKernelTunables = true;
                ProtectControlGroups = true;
                ProtectKernelLogs = true;
                ProtectHostname = true;
                ProtectClock = true;

                MemoryDenyWriteExecute = true;
                LockPersonality = true;

                ProcSubset = "pid";
                ProtectProc = "invisible";

                RestrictNamespaces = true;
                PrivateMounts = true;

                RestrictAddressFamilies = "AF_UNIX";
                PrivateNetwork = false;

                LimitNOFILE = 65536;
                RestrictRealtime = true;

                UMask = "0077";
              };
        };
    })
  ];
}
