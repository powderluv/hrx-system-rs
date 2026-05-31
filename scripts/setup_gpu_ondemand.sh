#!/usr/bin/env bash
# Configure the host so NEITHER amdgpu NOR vfio-pci claims the GPU at boot.
# After a (cold) reboot the GPU c3:00.0 is left unbound, and you load whichever
# driver you need on demand with gpu_bind.sh (amdgpu for HIP, vfio for VMs).
#
#   sudo bash setup_gpu_ondemand.sh enable    # then COLD power-cycle
#   sudo bash setup_gpu_ondemand.sh restore   # undo (back to vfio-at-boot)
#   bash setup_gpu_ondemand.sh status
#
# Rationale: live driver hand-offs on this gfx1201 card are unreliable and can
# wedge it; starting from an unbound-at-boot, never-touched device lets amdgpu
# (or vfio) init cleanly on demand.
set -euo pipefail
MANAGED=/etc/modprobe.d/hrx-gpu-ondemand.conf
ACTION="${1:-status}"

case "$ACTION" in
  enable)
    # Neutralize any vfio-pci id-claims and softdeps so vfio grabs nothing at boot.
    for f in /etc/modprobe.d/*.conf; do
      [[ "$f" == *.bak ]] && continue
      cp -n "$f" "$f.hrxod.bak" 2>/dev/null || true
      sed -i -E 's/^[[:space:]]*(options[[:space:]]+vfio-pci[[:space:]]+ids=)/#hrxod \1/; s/^[[:space:]]*(softdep[[:space:]])/#hrxod \1/' "$f"
    done
    # Blacklist both modules (explicit `modprobe` still loads them on demand).
    cat > "$MANAGED" <<'EOF'
# Managed by hrx-system-rs (setup_gpu_ondemand.sh).
# Keep the GPU unbound at boot; load amdgpu or vfio-pci on demand.
blacklist amdgpu
blacklist vfio-pci
blacklist vfio_pci
EOF
    update-initramfs -u
    echo "Done. COLD power-cycle, then: bash scripts/gpu_bind.sh amdgpu"
    ;;
  restore)
    rm -f "$MANAGED"
    for b in /etc/modprobe.d/*.hrxod.bak; do [[ -e "$b" ]] && cp "$b" "${b%.hrxod.bak}"; done
    update-initramfs -u
    echo "Restored. Reboot to return to the original (vfio-at-boot) config."
    ;;
  status|*)
    echo "managed_file=$([[ -e $MANAGED ]] && echo present || echo absent)"
    echo "active_blacklist_amdgpu=$(grep -rhE '^[[:space:]]*blacklist[[:space:]]+amdgpu' /etc/modprobe.d/ 2>/dev/null | wc -l)"
    echo "active_blacklist_vfio=$(grep -rhE '^[[:space:]]*blacklist[[:space:]]+vfio' /etc/modprobe.d/ 2>/dev/null | wc -l)"
    echo "active_vfio_ids=$(grep -rhE '^[[:space:]]*options[[:space:]]+vfio-pci[[:space:]]+ids=' /etc/modprobe.d/ 2>/dev/null | wc -l)"
    ;;
esac
