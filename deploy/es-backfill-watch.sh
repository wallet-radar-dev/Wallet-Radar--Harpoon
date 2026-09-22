#!/usr/bin/env bash
# Стежить за ізольованим Etherscan-краулом (backfill2, 3-й ключ). Авто-рестарт з чекпойнту,
# Telegram-пінг при завершенні (complete) або перезапуску після падіння.
set -uo pipefail
ENVF=/opt/wallet-tracker/.env
KEYF=/opt/wallet-tracker/deploy/backfill.env
DIR=/opt/wallet-tracker/deploy
B=/opt/wallet-tracker/dataset/backfill
BF=/opt/wallet-tracker/backfill
LOG=$DIR/es-backfill-watch.log
RUNLOG=$BF/run-es.log
ST_DONE=$DIR/.es-backfill-done
get(){ grep -E "^$1=" "$ENVF" | head -1 | cut -d= -f2-; }
TOK=$(get TELEGRAM_BOT_TOKEN); CHAT=$(get TELEGRAM_CHAT_ID); K=$(grep -E "^BACKFILL_ETHERSCAN_KEYS=" "$KEYF" | head -1 | cut -d= -f2-)
TS=$(date -u +%Y-%m-%dT%H:%M:%SZ)
send(){ [ -z "$TOK" ] && return; curl -s --max-time 20 "https://api.telegram.org/bot$TOK/sendMessage" \
  --data-urlencode "chat_id=$CHAT" --data-urlencode "text=$1" -d parse_mode=HTML >/dev/null; }

counter=$(grep -oE '\[[0-9]+/[0-9]+\]' "$RUNLOG" 2>/dev/null | tail -1 | tr -d '[]')
hdr=$(grep -n 'wallets x' "$RUNLOG" 2>/dev/null | tail -1 | cut -d: -f1)
comp=$(grep -n '\[backfill\] complete' "$RUNLOG" 2>/dev/null | tail -1 | cut -d: -f1)
running=0; pgrep -x backfill2 >/dev/null 2>&1 && running=1
done_flag=0
[ -n "${comp:-}" ] && [ -n "${hdr:-}" ] && [ "$comp" -gt "$hdr" ] && done_flag=1
echo "$TS running=$running counter=${counter:-?} done=$done_flag" >> "$LOG"

if [ "$done_flag" -eq 1 ]; then
  if [ ! -f "$ST_DONE" ]; then touch "$ST_DONE"
    rows=$(wc -l < "$B/transfers.csv" 2>/dev/null || echo '?')
    send "$(printf '\xF0\x9F\x8F\x81 <b>Wallet Radar</b>: Etherscan-backfill (3-й ключ) ГОТОВИЙ (%s рядків, transfers.csv). Готово до звірки/підміни.' "$rows")"
  fi
  exit 0
fi
[ "$running" -eq 1 ] && exit 0
# не працює і не завершено -> рестарт з чекпойнту
cd "$BF"
APP_ROOT=/opt/wallet-tracker ETHERSCAN_API_KEYS="$K" CONCURRENCY=64 \
  OUT_FILE=$B/transfers.csv PROG_FILE=$B/_progress.csv \
  setsid ./target/release/backfill2 >> "$RUNLOG" 2>&1 < /dev/null &
send "$(printf '\xE2\x9A\xA0\xEF\xB8\x8F <b>Wallet Radar</b>: Etherscan-backfill був зупинений (%s) — перезапустив з чекпойнту.' "${counter:-?}")"
echo "$TS restarted" >> "$LOG"
