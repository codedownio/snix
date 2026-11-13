# Configure restic backups to S3-compatible storage.
#
# Conventions:
# - restic's cache lives in /var/backup/restic/cache
# - repository password lives in `config.age.secrets.restic-repository-password.path`
# - object storage credentials in `config.age.secrets.restic-bucket-credentials.path`
{
  config,
  lib,
  pkgs,
  ...
}:

let
  cfg = config.services.depot.restic;
  mkStringOption =
    default:
    lib.mkOption {
      inherit default;
      type = lib.types.str;
    };
in
{
  options.services.depot.restic = {
    enable = lib.mkEnableOption "the restic backups";
    bucketEndpoint = mkStringOption "garage3.cyberchaos.dev";
    bucketName = mkStringOption "snix-backups";
    bucketCredentials = mkStringOption config.age.secrets.restic-bucket-credentials.path;
    repository = mkStringOption config.networking.hostName;
    interval = mkStringOption "hourly";

    paths =
      with lib;
      mkOption {
        description = "Directories that should be backed up";
        type = types.listOf types.str;
      };

    exclude =
      with lib;
      mkOption {
        description = "Files that should be excluded from backups";
        type = types.listOf types.str;
        default = [ ];
      };
  };

  config = lib.mkIf cfg.enable {
    systemd.services.restic = {
      script = "${pkgs.restic}/bin/restic backup ${lib.concatStringsSep " " cfg.paths}";

      serviceConfig.ExecStartPre = pkgs.writeShellScript "init-repo" ''
        ${pkgs.restic}/bin/restic init && echo "Initializing the repository." || echo "Already initialized."
      '';

      environment = {
        RESTIC_REPOSITORY = "s3:${cfg.bucketEndpoint}/${cfg.bucketName}/${cfg.repository}";
        AWS_SHARED_CREDENTIALS_FILE = cfg.bucketCredentials;
        RESTIC_PASSWORD_FILE = config.age.secrets.restic-repository-password.path;
        RESTIC_CACHE_DIR = "/var/backup/restic/cache";

        RESTIC_EXCLUDE_FILE = builtins.toFile "exclude-files" (lib.concatStringsSep "\n" cfg.exclude);
      };
    };

    systemd.timers.restic = {
      wantedBy = [ "multi-user.target" ];
      timerConfig.OnCalendar = cfg.interval;
    };

    environment.systemPackages = [ pkgs.restic ];
  };
}
