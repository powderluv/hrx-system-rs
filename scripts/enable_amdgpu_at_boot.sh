#!/usr/bin/env bash
# Make amdgpu (not vfio-pci) claim the gfx1201 GPU at the NEXT boot, by disabling
# the VFIO passthrough config on this host. This is the reliable way to get a
# working GPU: live bind/unbind/reset of this card wedges it (see README).
#
# On Shark-A the relevant files are:
#   /etc/modprobe.d/blacklist-amdgpu.conf       (blacklist amdgpu)
#   /etc/modprobe.d/blacklist-amdgpu-temp.conf  (blacklist amdgpu)
#   /etc/modprobe.d/vfio-pci-temp.conf          (options vfio-pci ids=1002:7551,1002:ab40)
# plus kernel cmdline rd.driver.pre=vfio-pci (harmless once ids are gone).
#
#   sudo bash enable_amdgpu_at_boot.sh enable    # then COLD power-cycle
#   sudo bash enable_amdgpu_at_boot.sh restore   # then reboot (back to VFIO)
#   bash enable_amdgpu_at_boot.sh status
#
# NOTE: requires a reboot (ideally a full power-off) to take effect. Does NOT
# reboot for you. A cold boot is recommended because a wedged GPU may not be
# cleared by a warm reboot.
set -euo pipefail
ACTION="${1:-status}"
FILES=(/etc/modprobe.d/blacklist-amdgpu.conf
       /etc/modprobe.d/blacklist-amdgpu-temp.conf
       /etc/modprobe.d/vfio-pci-temp.conf)

case "$ACTION" in
  enable)
    for f in "${FILES[@]}"; do
      [[ -f "$f" ]] || continue
      cp -n "$f" "$f.hrx.bak"
      # Comment out any blacklist-amdgpu and vfio-pci id-claim lines.
      sed -i -E 's/^[[:space:]]*(blacklist[[:space:]]+amdgpu)/#\1/; s/^[[:space:]]*(options[[:space:]]+vfio-pci[[:space:]]+ids=)/#\1/' "$f"
      echo "patched $f"
    done
    sudo update-initramfs -u
    echo "Done. COLD power-cycle the host. Then verify (sandbox disabled):"
    echo "  ls /dev/kfd && rocminfo | grep -m1 gfx && ./build/test-out/hip_smoke"
    ;;
  restore)
    for f in "${FILES[@]}"; do
      [[ -f "$f.hrx.bak" ]] && cp "$f.hrx.bak" "$f" && echo "restored $f"
    done
    sudo update-initramfs -u
    echo "Restored. Reboot to return the GPU to vfio-pci for VM passthrough."
    ;;
  status|*)
    for f in "${FILES[@]}"; do
      echo "== $f =="; grep -nE 'blacklist|vfio-pci|ids=' "$f" 2>/dev/null || echo "(absent)"
    done
    ;;
esac
