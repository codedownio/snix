{
  depot,
  pkgs,
  lib,
  ...
}: # readTree options
{ config, ... }: # passed by module system

let
  mod = name: depot.path.origSrc + ("/ops/modules/" + name);

in
{
  imports = [
    ./nar-bridge.nix
    depot.third_party.srvos.nixosModules.hardware-hetzner-online-intel
    depot.third_party.srvos.nixosModules.mixins-nginx

    # Automatically enable metric and log collection.
    (mod "o11y/alloy.nix")
    (mod "www/nixos.snix.store.nix")
    (mod "www/cache-updates.snix.store.nix")

    (depot.third_party.agenix.src + "/modules/age.nix")
  ];

  config = {
    nixpkgs.hostPlatform = lib.mkForce "x86_64-linux";

    networking.hostName = "snix-cache";

    boot.loader.efi.canTouchEfiVariables = true;
    boot.loader.systemd-boot.configurationLimit = 10;
    boot.loader.systemd-boot.enable = true;
    boot.loader.timeout = 3;
    boot.supportedFilesystems = [ "btrfs" ];

    # Disk /dev/nvme0n1: 1024 GB (=> 953 GiB)
    # Disk /dev/nvme1n1: 1024 GB (=> 953 GiB)
    # Disk /dev/sda: 6001 GB (=> 5589 GiB)
    # Disk /dev/sdb: 6001 GB (=> 5589 GiB)
    # btrfs raid1 on two SSDs, btrfs raid0 (data) on HDDs.
    fileSystems."/" = {
      fsType = "btrfs";
      device = "/dev/disk/by-label/root";
      options = [
        "compress=zstd"
        "discard"
      ];
    };
    fileSystems."/boot" = {
      fsType = "vfat";
      device = "/dev/disk/by-partlabel/esp"; # ef00
    };
    fileSystems."/tank" = {
      fsType = "btrfs";
      device = "/dev/disk/by-label/tank";
      options = [ "discard" ];
    };

    systemd.network.networks."10-uplink".networkConfig.Address = "2a01:4f9:2a:2597::2/64";

    # Enable SSH and add some keys
    services.openssh.enable = true;

    users.users.root.openssh.authorizedKeys.keys = depot.ops.users.flokli ++ depot.ops.users.picnoir;

    environment.systemPackages = [
      pkgs.helix
      pkgs.htop
      pkgs.kitty.terminfo
      pkgs.tmux
    ];

    system.stateVersion = "24.11";
  };
}
