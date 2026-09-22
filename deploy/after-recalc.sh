#!/bin/bash
# чекає завершення перерахунку (after-edges2.sh) і будує картину по хопах
cd /opt/wallet-tracker
for i in $(seq 1 720); do grep -q "ГОТОВО" /tmp/after-edges2.log 2>/dev/null && break; sleep 30; done
GROUP=NET TOPN=5 ./nextaction/target/release/hopview /opt/wallet-tracker > /tmp/hopview.log 2>&1
echo "[after-recalc] хоп-картина готова $(date -u +%H:%M:%S)"
