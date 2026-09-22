#!/usr/bin/env bash
# Авто-оновлення strategy-short.json + strategy-volatility.json.
# Telegram-пінг, коли:
#   - у SHORT-список потрапляє НОВИЙ токен (кити щойно залили USD-накопичення);
#   - у ВОЛАТИЛЬНИЙ список потрапляє НОВИЙ токен (модель прогнозує великий ±рух).
set -uo pipefail
ROOT=/opt/wallet-tracker
BIN=$ROOT/nextaction/target/release/strategy
VOLBIN=$ROOT/nextaction/target/release/volatility
JSON=$ROOT/dataset/strategy-short.json
VJSON=$ROOT/dataset/strategy-volatility.json
PREV=$ROOT/deploy/.strategy-prev.txt
VPREV=$ROOT/deploy/.volatility-prev.txt
LOG=$ROOT/deploy/strategy-refresh.log
ENVF=$ROOT/.env
MM=$ROOT/dataset/nextaction/mm-whales.txt
get(){ grep -E "^$1=" "$ENVF" | head -1 | cut -d= -f2-; }
TOK=$(get TELEGRAM_BOT_TOKEN); CHAT=$(get TELEGRAM_CHAT_ID)
TS=$(date -u +%Y-%m-%dT%H:%M:%SZ)
[ -x "$BIN" ] || { echo "$TS [err] strategy bin відсутній" >> "$LOG"; exit 1; }
send(){ [ -n "$TOK" ] && [ -n "$CHAT" ] || return; curl -s --max-time 20 "https://api.telegram.org/bot$TOK/sendMessage" --data-urlencode "chat_id=$CHAT" --data-urlencode "text=$1" -d parse_mode=HTML >/dev/null; }

# --- SHORT ---
MM_EXCLUDE=$MM "$BIN" >/dev/null 2>&1
cnt=$(jq -r ".count // 0" "$JSON" 2>/dev/null)
cur=$(jq -r ".tokens[] | \"\(.chain):\(.token)\"" "$JSON" 2>/dev/null | sort)
echo "$TS strategy-short оновлено: count=$cnt" >> "$LOG"
if [ -f "$PREV" ]; then
  new=$(comm -13 "$PREV" <(echo "$cur") 2>/dev/null || true)
  [ -n "$new" ] && send "$(printf 'Wallet Radar: НОВІ SHORT-кандидати (кити заливають USD-накопичення):\n%s' "$new")"
fi
echo "$cur" > "$PREV"

# --- ВОЛАТИЛЬНІСТЬ ---
if [ -x "$VOLBIN" ]; then
  MM_EXCLUDE=$MM "$VOLBIN" "$ROOT" >/dev/null 2>&1
  vcnt=$(jq -r ".count // 0" "$VJSON" 2>/dev/null)
  vcur=$(jq -r ".tokens[] | \"\(.chain):\(.token)\"" "$VJSON" 2>/dev/null | sort)
  echo "$TS strategy-volatility оновлено: count=$vcnt" >> "$LOG"
  if [ -f "$VPREV" ]; then
    vnew=$(comm -13 "$VPREV" <(echo "$vcur") 2>/dev/null || true)
    if [ -n "$vnew" ]; then
      # долучаємо pBigMove/символ для нових ключів
      lines=$(printf '%s\n' "$vnew" | while IFS=: read -r ch tk; do
        jq -r --arg ch "$ch" --arg tk "$tk" '.tokens[] | select(.chain==$ch and .token==$tk) | "\(.symbol // .token) [\(.chain)] P=\(.pBigMove)"' "$VJSON" 2>/dev/null
      done)
      [ -n "$lines" ] && send "$(printf 'Wallet Radar: НОВІ ВОЛАТИЛЬНІ кандидати (кити тяжко набирають -> очікуй великий ±рух, straddle):\n%s' "$lines")"
    fi
  fi
  echo "$vcur" > "$VPREV"
fi
