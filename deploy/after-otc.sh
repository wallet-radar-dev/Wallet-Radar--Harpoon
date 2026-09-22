#!/bin/bash
# після пошуку OTC — лаг-аналіз, потім спільний маркер завершення всього циклу
cd /opt/wallet-tracker
for i in $(seq 1 720); do grep -q OTC_DONE /tmp/otc.log 2>/dev/null && break; sleep 60; done
echo "[lag] старт $(date -u +%H:%M)" >> /tmp/lag.log
CHAIN=1 ./nextaction/target/release/entrylag /opt/wallet-tracker >> /tmp/lag.log 2>&1
echo "LAG_DONE" >> /tmp/lag.log
# чекаємо ще й повний ланцюг збору (mainnet + arbitrum з індексами)
for i in $(seq 1 720); do grep -q TXCHAIN_DONE /tmp/txchain.log 2>/dev/null && break; sleep 60; done
echo "ALL_DONE $(date -u +%H:%M)" >> /tmp/all.log
