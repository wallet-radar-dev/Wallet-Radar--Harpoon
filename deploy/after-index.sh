#!/bin/bash
# щойно буде індекс mainnet — запускає пошук OTC-угод
cd /opt/wallet-tracker
for i in $(seq 1 480); do grep -q "індекс mainnet готовий" /tmp/txchain.log 2>/dev/null && break; sleep 60; done
echo "[otc] старт $(date -u +%H:%M)" >> /tmp/otc.log
CHAIN=1 MIN_USD=25000 WIN_H=12 ./nextaction/target/release/otcfind /opt/wallet-tracker >> /tmp/otc.log 2>&1
echo "OTC_DONE" >> /tmp/otc.log
