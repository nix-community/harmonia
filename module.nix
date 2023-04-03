{ config, pkgs, lib, ... }:
let
  cfg = config.services.harmonia;

  format = pkgs.formats.toml { };
  configFile = format.generate "harmonia.toml" cfg.settings;
in
{
  options = {
    services.harmonia = {
      enable = lib.mkEnableOption "Harmonia: Nix binary cache written in Rust";

      settings = lib.mkOption {
        type = lib.types.submodule {
          freeformType = format.type;
        };

        description = "Settings to merge with the default configuration";
      };
    };
  };

  config = lib.mkIf cfg.enable {
    services.harmonia.settings = builtins.mapAttrs (_: v: lib.mkDefault v) {
      bind = "[::]:5000";
      workers = 4;
      max_connection_rate = 256;
      priority = 50;
    };

    environment.systemPackages = [ pkgs.harmonia ];

    systemd.services.harmonia = {
      description = "harmonia binary cache service";

      requires = [ "nix-daemon.socket" ];
      after = [ "network.target" ];
      wantedBy = [ "multi-user.target" ];

      path = [ config.nix.package.out ];
      environment = {
        NIX_REMOTE = "daemon";
        LIBEV_FLAGS = "4"; # go ahead and mandate epoll(2)
        CONFIG_FILE = lib.mkIf (configFile != null) configFile;
        RUST_LOG = "info";
      };

      # Note: it's important to set this for nix-store, because it wants to use
      # $HOME in order to use a temporary cache dir. bizarre failures will occur
      # otherwise
      environment.HOME = "/run/harmonia";

      serviceConfig = {
        ExecStart = "${pkgs.harmonia}/bin/harmonia";

        User = "harmonia";
        Group = "harmonia";

        RuntimeDirectory = "harmonia";

        SystemCallFilter = "@system-service";

        PrivateNetwork = false;

        LimitNOFILE = 65536;
      };
    };

    users = {
      users.harmonia = {
        isSystemUser = true;
        group = "harmonia";
      };
      groups.harmonia = { };
    };
  };
}
