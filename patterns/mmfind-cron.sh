#!/bin/sh
# Щотижневий рефреш ончейн-ММ (mmfind.json, який споживає mmsignal).
cd /opt/wallet-tracker/dataset/backfill || exit 1
export PATH=$PATH:/home/ubuntu/.cargo/bin
LOG=/opt/wallet-tracker/patterns/mmfind-cron.log
echo "=== $(date -u +%FT%TZ) mmfind refresh ===" >> "$LOG"
# NET-члени з авторитетного членства (grouppart) -> net-members.txt (дефолт mmfind)
python3 - >> "$LOG" 2>&1 <<'PY'
import json
d=json.load(open('group-membership.json'))['wallets']
net=[a for a,x in d.items() if x.get('group')=='NET']
open('net-members.txt','w').write('\n'.join(net))
print('NET members:', len(net))
PY
# рефреш (дефолт wallet-file = net-members.txt)
timeout 700 /opt/wallet-tracker/backfill/target/release/mmfind /opt/wallet-tracker >> "$LOG" 2>&1
echo "mmfind.json refreshed" >> "$LOG"
