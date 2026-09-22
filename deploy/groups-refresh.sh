#!/usr/bin/env bash
# Щоденне оновлення координованих груп (coordination bin на повному backfill).
set -uo pipefail
ROOT=/opt/wallet-tracker
LOG=$ROOT/deploy/groups-refresh.log
"$ROOT/nextaction/target/release/coordination" "$ROOT" >/dev/null 2>&1
n=$(jq -r '.totalGroups // "?"' "$ROOT/dataset/wallet-groups.json" 2>/dev/null || echo '?')
echo "$(date -u +%Y-%m-%dT%H:%M:%SZ) wallet-groups оновлено: $n груп" >> "$LOG"
