#!/usr/bin/env bash
# Try to bind the host GPU to amdgpu so HIP can use it for testing.
# Host: Shark-A, RX 9070 XT (gfx1201) at 0000:c3:00.0. Needs sudo.
#
# IMPORTANT: on this host a LIVE vfio-pci -> amdgpu handoff FAILS. amdgpu's SMU
# comes up but the GFX ring test times out:
#     [drm] *ERROR* ring gfx_0.0.0 test failed (-110)
#     amdgpu: hw_init of IP block <gfx_v12_0> failed -110
#     amdgpu: Fatal error during GPU init
# because the card is not power-cycled/reset when switching from vfio. KFD never
# comes up, so /dev/kfd is absent and HIP sees 0 devices.
#
# The reliable way to get a working GPU is to let amdgpu claim it from a clean
# boot (see enable_amdgpu_at_boot.sh), which is the documented working path
# (gfx1201 native amdgpu). This script only attempts the live rebind and
# reports whether it actually produced a usable KFD device.
set -euo pipefail
DEV="${GPU_BDF:-0000:c3:00.0}"

cur="$(basename "$(readlink -f "/sys/bus/pci/devices/$DEV/driver" 2>/dev/null)" 2>/dev/null || true)"
echo "current driver: ${cur:-none}"
if [[ "$cur" != "amdgpu" ]]; then
  sudo modprobe amdgpu
  [[ "$cur" == vfio-pci ]] && echo "$DEV" | sudo tee /sys/bus/pci/drivers/vfio-pci/unbind >/dev/null
  echo amdgpu | sudo tee "/sys/bus/pci/devices/$DEV/driver_override" >/dev/null
  echo "$DEV" | sudo tee /sys/bus/pci/drivers/amdgpu/bind >/dev/null 2>/dev/null || true
  sleep 4
fi

if [[ -e /dev/kfd ]]; then
  echo "OK: /dev/kfd present — GPU usable for HIP compute"
else
  echo "FAILED: /dev/kfd absent. amdgpu GFX init did not complete on live rebind."
  echo "Check: sudo dmesg | grep -i 'ring gfx\\|Fatal error during GPU init'"
  echo "Use enable_amdgpu_at_boot.sh + reboot for a clean amdgpu bind."
  exit 1
fi
