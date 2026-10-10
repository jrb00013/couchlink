#!/usr/bin/env bash
# Toggle the PPU-priority fix for BO2 (the 'PPU Reservation Priority Over SPUs' option in the per-game config).
#   tools/rpcs3-debug/set-prio.sh on|off|status      (relaunch BO2 afterwards; the config is read at boot)
set -uo pipefail
cfg="${RPCS3_DIR:-/mnt/c/Users/josep/RPCS3}/config/custom_configs/config_BLUS31011.yml"
cur() { tr -d '\r' < "$cfg" | sed -n 's/^  PPU Reservation Priority Over SPUs: //p' | head -1; }
case "${1:-status}" in
  on)  sed -i 's/^  PPU Reservation Priority Over SPUs:.*/  PPU Reservation Priority Over SPUs: true/' "$cfg";;
  off) sed -i 's/^  PPU Reservation Priority Over SPUs:.*/  PPU Reservation Priority Over SPUs: false/' "$cfg";;
  status) ;;
  *) echo "usage: $0 on|off|status"; exit 2;;
esac
echo "PPU Reservation Priority Over SPUs = $(cur)  (fix is $([ "$(cur)" = true ] && echo ON || echo OFF); relaunch BO2 to apply)"
