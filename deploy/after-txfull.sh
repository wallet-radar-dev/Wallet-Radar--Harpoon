#!/bin/bash
# ланцюг збору бази: mainnet -> індекс -> Arbitrum -> індекс. Працює без участі сесії.
cd /opt/wallet-tracker
while pgrep -f "release/txfull" >/dev/null; do sleep 60; done
echo "[chain] mainnet зібрано $(date -u +%H:%M)" >> /tmp/txchain.log
CHAIN=1 ./nextaction/target/release/txindex /opt/wallet-tracker >> /tmp/txchain.log 2>&1
echo "[chain] індекс mainnet готовий" >> /tmp/txchain.log
CHAIN=42161 CONC=10 ./backfill/target/release/txfull /opt/wallet-tracker > /tmp/txfull-42161.log 2>&1
echo "[chain] arbitrum зібрано $(date -u +%H:%M)" >> /tmp/txchain.log
CHAIN=42161 ./nextaction/target/release/txindex /opt/wallet-tracker >> /tmp/txchain.log 2>&1
echo "TXCHAIN_DONE" >> /tmp/txchain.log
