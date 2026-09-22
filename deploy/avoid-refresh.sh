#!/usr/bin/env bash
# Авто-оновлення whale-avoid.json: перезапускає avoid bin на свіжому transfers.csv (backfill живий).
# Бонус: Telegram-пінг, коли в AVOID-список потрапляє НОВИЙ токен (кит щойно залив емісію).
set -uo pipefail
ROOT=/opt/wallet-tracker
BIN=$ROOT/nextaction/target/release/avoid
AVJSON=$ROOT/dataset/whale-avoid.json
PREV=$ROOT/deploy/.avoid-prev.txt
LOG=$ROOT/deploy/avoid-refresh.log
ENVF=$ROOT/.env
get(){ grep -E "^$1=" "$ENVF" | head -1 | cut -d= -f2-; }
TOK=$(get TELEGRAM_BOT_TOKEN); CHAT=$(get TELEGRAM_CHAT_ID)
TS=$(date -u +%Y-%m-%dT%H:%M:%SZ)

[ -x "$BIN" ] || { echo "$TS [err] avoid bin відсутній" >> "$LOG"; exit 1; }
"$BIN" >/dev/null 2>&1
cnt=$(jq -r ".count // 0" "$AVJSON" 2>/dev/null)
cur=$(jq -r ".tokens[] | \"\(.chain):\(.token)\"" "$AVJSON" 2>/dev/null | sort)
echo "$TS whale-avoid оновлено: count=$cnt" >> "$LOG"

send(){
  [ -n "$TOK" ] && [ -n "$CHAT" ] || return
  curl -s --max-time 20 "https://api.telegram.org/bot$TOK/sendMessage" \
    --data-urlencode "chat_id=$CHAT" --data-urlencode "text=$1" -d parse_mode=HTML >/dev/null \
    && echo "$TS [tg] sent" >> "$LOG"
}

# НОВІ токени (у cur, яких не було в PREV) -> пінг
if [ -f "$PREV" ] && [ -n "$cur" ]; then
  new=$(comm -23 <(printf "%s\n" "$cur") <(sort "$PREV") 2>/dev/null)
  if [ -n "$new" ]; then
    msg=$(printf "%s\n" "$new" | while IFS= read -r ct; do
      [ -z "$ct" ] && continue
      c="${ct%%:*}"; t="${ct#*:}"
      jq -r --arg c "$c" --arg t "$t" ".tokens[] | select(.chain==\$c and .token==\$t) | \"\(.symbol) (\(.chain)) — \(.supplySharePct)% емісії\"" "$AVJSON" 2>/dev/null
    done)
    [ -n "$msg" ] && send "$(printf "\xE2\x9A\xA0\xEF\xB8\x8F <b>Wallet Radar</b>: НОВІ AVOID-токени (кити залили емісію):\n%s\nІсторично ~66%% таких недоперформують ринок за 14д." "$msg")"
  fi
fi
printf "%s\n" "$cur" > "$PREV"
