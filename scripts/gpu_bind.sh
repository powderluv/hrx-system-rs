#!/usr/bin/env bash
# Bind the GPU to a driver on demand (after a boot where it was left unbound by
# setup_gpu_ondemand.sh). Run BEFORE any process has touched the GPU for a clean
# init.
#
#   sudo bash gpu_bind.sh amdgpu   # for HIP/ROCm
#   sudo bash gpu_bind.sh vfio     # for VM passthrough
#   bash gpu_bind.sh status
set -euo pipefail
DEV="${GPU_BDF:-0000:c3:00.0}"
AUDIO="${GPU_AUDIO_BDF:-0000:c3:00.1}"
ACTION="${1:-status}"

cur() { basename "$(readlink -f "/sys/bus/pci/devices/$1/driver" 2>/dev/null)" 2>/dev/null || echo none; }

unbind() { # $1=bdf
  local d; d="$(cur "$1")"
  [[ "$d" != none ]] && echo "$1" | tee "/sys/bus/pci/drivers/$d/unbind" >/dev/null || true
}

case "$ACTION" in
  amdgpu)
    [[ "$(cur "$DEV")" == amdgpu ]] && { echo "already amdgpu"; exit 0; }
    unbind "$DEV"
    echo | tee "/sys/bus/pci/devices/$DEV/driver_override" >/dev/null
    modprobe amdgpu
    echo "$DEV" | tee /sys/bus/pci/drivers/amdgpu/bind >/dev/null 2>/dev/null || true
    sleep 4
    if [[ -e /dev/kfd && "$(cur "$DEV")" == amdgpu ]]; then
      echo "OK: amdgpu bound, /dev/kfd present"
    else
      echo "FAILED: check 'sudo dmesg | grep -i amdgpu | tail'"; exit 1
    fi
    ;;
  vfio)
    modprobe vfio-pci
    for b in "$DEV" "$AUDIO"; do
      unbind "$b"
      echo vfio-pci | tee "/sys/bus/pci/devices/$b/driver_override" >/dev/null
      echo "$b" | tee /sys/bus/pci/drivers/vfio-pci/bind >/dev/null 2>/dev/null || true
    done
    echo "GPU driver now: $(cur "$DEV"); audio: $(cur "$AUDIO")"
    ;;
  status|*)
    echo "gpu($DEV) driver: $(cur "$DEV")"
    echo "audio($AUDIO) driver: $(cur "$AUDIO")"
    echo "kfd: $([[ -e /dev/kfd ]] && echo present || echo absent)"
    ;;
esac
