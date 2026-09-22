#!/usr/bin/env bash
# Крок B: історія денних цін (DefiLlama) для токенів-кандидатів (де сидять кити).
# Вхід:  dataset/nextaction/candidates.tsv  (chain,address,symbol,holders,distr,whales)
# Вихід: dataset/nextaction/prices/{chain}_{address}.json  ({symbol,decimals,prices[]})
# Резюмиться (пропускає вже зібрані). STEP_B_TEST=1 -> не шле Telegram (для тесту).
set -uo pipefail
ROOT=/opt/wallet-tracker
ENVF=$ROOT/.env
ND=$ROOT/dataset/nextaction
LIST=$ND/candidates.tsv
OUT=$ND/prices
LOG=$ROOT/deploy/step-b-prices.log
SPAN=500   # максимум точок за один запит DefiLlama (~16 міс денних цін)
mkdir -p "$OUT"
get(){ grep -E "^$1=" "$ENVF" | head -1 | cut -d= -f2-; }
TOK=$(get TELEGRAM_BOT_TOKEN); CHAT=$(get TELEGRAM_CHAT_ID)
ts(){ date -u +%Y-%m-%dT%H:%M:%SZ; }

# наші назви мереж -> DefiLlama-слаги (решта збігається)
llama_chain(){
  case "$1" in
    eth) echo ethereum;;
    zksync) echo era;;
    *) echo "$1";;
  esac
}

send(){
  [ -n "${STEP_B_TEST:-}" ] && { echo "$(ts) [test] skip send: $1" >> "$LOG"; return; }
  [ -n "$TOK" ] && [ -n "$CHAT" ] || return
  curl -s --max-time 20 "https://api.telegram.org/bot$TOK/sendMessage" \
    --data-urlencode "chat_id=$CHAT" --data-urlencode "text=$1" -d parse_mode=HTML >/dev/null
}

[ -f "$LIST" ] || { echo "$(ts) НЕМАЄ $LIST — Крок A (netflow --list) не відпрацював" >> "$LOG"; exit 1; }

echo "$(ts) Крок B старт (список: $(wc -l < "$LIST") рядків)" >> "$LOG"
total=0; ok=0; skip=0; miss=0
while IFS=$'\t' read -r chain address symbol rest; do
  [ "$chain" = "chain" ] && continue
  [ -z "$address" ] && continue
  total=$((total+1))
  f="$OUT/${chain}_${address}.json"
  [ -s "$f" ] && { skip=$((skip+1)); continue; }
  lc=$(llama_chain "$chain")
  key="${lc}:${address}"
  resp=$(curl -s --max-time 25 "https://coins.llama.fi/chart/${key}?period=1d&span=${SPAN}")
  n=$(echo "$resp" | jq -r --arg k "$key" '.coins[$k].prices | length' 2>/dev/null)
  if [ -n "$n" ] && [ "$n" != "null" ] && [ "$n" -gt 0 ] 2>/dev/null; then
    echo "$resp" | jq -c --arg k "$key" '.coins[$k] | {symbol, decimals, prices}' > "$f" 2>/dev/null && ok=$((ok+1))
  else
    miss=$((miss+1))
  fi
  sleep 0.3
done < "$LIST"

echo "$(ts) Крок B готово: total=$total ok=$ok skip=$skip miss=$miss" >> "$LOG"
send "$(printf '\xF0\x9F\x92\xB0 <b>Wallet Radar</b>: Крок B готовий.\nЦіни: %s зібрано, %s без ціни, %s всього.\nКрок C (тест сигналу whale-flow -> ціна) потребує аналізу.' "$ok" "$miss" "$total")"
