#!/bin/bash
# чекає завершення краулу eth-edges2 і одразу переганяє повний ланцюг:
# розбиття на групи -> реальні баланси ETH -> капітал на реальній ліквідності
cd /opt/wallet-tracker
PID=$(pgrep -f "release/eth-edges2" | head -1)
[ -n "$PID" ] && while kill -0 "$PID" 2>/dev/null; do sleep 30; done
echo "[after-edges2] краул завершено $(date -u +%H:%M:%S), запускаю перерахунок"
WRITE=1 ./nextaction/target/release/grouppart /opt/wallet-tracker > /tmp/gp-post.log 2>&1
CODE_MIN=5 ./backfill/target/release/ethbal /opt/wallet-tracker > /tmp/eb-post.log 2>&1
WRITE=1 ./nextaction/target/release/capreal /opt/wallet-tracker > /tmp/cr-post.log 2>&1
echo "[after-edges2] ГОТОВО $(date -u +%H:%M:%S)"
