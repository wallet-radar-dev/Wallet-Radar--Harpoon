#!/bin/bash
# Autonomous analytics refresh — regenerates the derived datasets (features,
# clusters, sector rotation, whale families, family enrichment) from the current
# backfill. Local-only (no network), runs in seconds. Driven by a systemd timer.
export APP_ROOT=/opt/wallet-tracker
cd /opt/wallet-tracker/features || exit 1
BIN=./target/release
echo "=== $(date -u) analytics refresh start ==="
"$BIN/features"    2>&1 | tail -1 || echo "features FAILED"
"$BIN/cluster"     2>&1 | tail -1 || echo "cluster FAILED"
"$BIN/rotation"    2>&1 | tail -1 || echo "rotation FAILED"
HUB_PCT=0.90 MIN_EDGE=2 "$BIN/families" 2>&1 | tail -1 || echo "families FAILED"
"$BIN/famfeatures" 2>&1 | tail -1 || echo "famfeatures FAILED"
"$BIN/famcluster" 2>&1 | tail -1 || echo "famcluster FAILED"
echo "=== $(date -u) analytics refresh done ==="
