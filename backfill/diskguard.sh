#!/bin/bash
# Disk guard: if free space on / drops below 8G, stop ALL backfill to protect
# prod (disk-full crashes it). Kills the systemd service AND any manual backfill2
# (user runs it in parallel). Backfill is resumable, so a stop is always safe.
while true; do
  fkb=$(df --output=avail / | tail -1 | tr -d ' ')
  fgb=$((fkb/1024/1024))
  if [ "$fgb" -lt 8 ]; then
    sudo systemctl stop wallet-backfill.service 2>/dev/null
    pkill -9 -x backfill2 2>/dev/null
    pkill -9 -x backfill 2>/dev/null
    echo "$(date) DISK GUARD: free ${fgb}G < 8G -> stopped ALL backfill (systemd + manual backfill2)" >> /opt/wallet-tracker/dataset/backfill/diskguard.log
  fi
  sleep 180
done
