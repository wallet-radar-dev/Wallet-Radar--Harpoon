#!/usr/bin/env bash
# BTC whale-універсум: цикл collect+expand. Нарощує граф китів. Env: ROUNDS(5) MIN_BTC(50) MAX_TXS(1200)
set -uo pipefail
ROOT=/opt/wallet-tracker
LOG=$ROOT/deploy/btc-grow.log
R=${ROUNDS:-5}
for i in $(seq 1 $R); do
  echo "$(date -u +%H:%M:%S) === round $i: collect ===" >> "$LOG"
  MAX_TXS=${MAX_TXS:-1200} "$ROOT/backfill/target/release/btc-collect" "$ROOT" >> "$LOG" 2>&1
  echo "$(date -u +%H:%M:%S) === round $i: expand ===" >> "$LOG"
  MIN_BTC=${MIN_BTC:-50} MAX_TX=50000 "$ROOT/backfill/target/release/btc-expand" "$ROOT" >> "$LOG" 2>&1
  n=$(grep -vc '^#' "$ROOT/dataset/backfill/btc-wallets.txt")
  echo "$(date -u +%H:%M:%S) round $i done: seed=$n адрес, btc-рядків=$(wc -l < $ROOT/dataset/backfill/transfers-btc.csv)" >> "$LOG"
done
echo "$(date -u +%H:%M:%S) GROWTH готово" >> "$LOG"
