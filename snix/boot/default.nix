{ pkgs, ... }:

let
  # A NixOS module turning a system into something bootable from a snix-store
  # served over virtiofs, using NixOS' systemd-initrd.
  snixGuest =
    {
      lib,
      pkgs,
      modulesPath,
      ...
    }:
    {
      # Provides common virtio kernel modules.
      imports = [ "${modulesPath}/profiles/qemu-guest.nix" ];

      boot = {
        loader.grub.enable = false;
        # cloud-hypervisor exposes the serial port as ttyS0 on x86_64, and as
        # ttyAMA0 (PL011) on aarch64.
        kernelParams = [
          "console=${if pkgs.stdenv.hostPlatform.isAarch64 then "ttyAMA0" else "ttyS0"}"
        ];
      };

      fileSystems = {
        "/" = {
          fsType = "tmpfs";
          options = [
            "defaults"
            "mode=0755"
          ];
          neededForBoot = true;
        };

        "/nix/store" = {
          device = "snix";
          fsType = "virtiofs";
          options = [ "ro" ];
          neededForBoot = true;
        };
      };

      # switch-root needs an os-release on the target root.
      boot.initrd.systemd.tmpfiles.settings."10-snix-os-release"."/sysroot/etc/os-release".f = {
        mode = "0644";
        argument = "ID=snix";
      };

      # Speed up evaluation/builds.
      documentation.enable = false;
      system.stateVersion = lib.mkDefault "26.05";
    };

  # A NixOS system providing the kernel and initrd
  # that `run-snix-vm` boots. Its own toplevel is the default `init=`, but any store
  # path can be selected via the cmdline (see run-snix-vm / README).
  system = pkgs.nixos snixGuest;

  kernelImage =
    # cloud-hypervisor boots the PVH ELF entry (vmlinux) on x86_64.
    if pkgs.stdenv.hostPlatform.isx86_64 then
      "${system.config.boot.kernelPackages.kernel.dev}/vmlinux"
    else
      "${system.config.boot.kernelPackages.kernel}/${system.config.system.boot.loader.kernelFile}";

  initrd = "${system.config.system.build.initialRamdisk}/${system.config.system.boot.loader.initrdFile}";
in
{
  inherit snixGuest;

  # Start a `snix-store virtiofs` daemon from $PATH, then a cloud-hypervisor
  # pointed at it, booting the NixOS kernel with systemd-initrd.

  # Supports the following env vars (and defaults)
  # CH_NUM_CPUS=2
  # CH_MEM_SIZE=512M
  # CH_CMDLINE=""
  run-snix-vm = pkgs.writeShellApplication {
    name = "run-snix-vm";
    runtimeInputs = [ pkgs.cloud-hypervisor ];
    text = ''
      tempdir=$(mktemp -d)

      cleanup() {
        [[ -n ''${virtiofsd_pid-} ]] && kill "$virtiofsd_pid" 2>/dev/null
        chmod -R u+rw "$tempdir" 2>/dev/null
        rm -rf "$tempdir"
      }
      trap cleanup EXIT

      # Spin up the virtiofs daemon
      snix-store virtiofs -l "$tempdir/snix.sock" &
      virtiofsd_pid=$!

      # Wait for the socket to exist.
      until [ -e "$tempdir/snix.sock" ]; do sleep 0.1; done

      CH_NUM_CPUS="''${CH_NUM_CPUS:-2}"
      CH_MEM_SIZE="''${CH_MEM_SIZE:-512M}"
      CH_CMDLINE="''${CH_CMDLINE:-}"

      # spin up cloud_hypervisor
      cloud-hypervisor \
        --cpus boot="$CH_NUM_CPUS" \
        --memory shared=on,size="$CH_MEM_SIZE" \
        --console null \
        --serial tty \
        --kernel ${kernelImage} \
        --initramfs ${initrd} \
        --cmdline "${toString system.config.boot.kernelParams} init=${system.config.system.build.toplevel}/init reboot=t panic=-1 $CH_CMDLINE" \
        --fs tag=snix,socket="$tempdir/snix.sock",num_queues=1,queue_size=512
    '';
  };

  meta.ci.targets = [
    "run-snix-vm"
  ];
}
