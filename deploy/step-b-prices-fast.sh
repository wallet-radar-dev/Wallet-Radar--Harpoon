#!/usr/bin/env bash
# Крок B (ПАРАЛЕЛЬНИЙ): ціни DefiLlama для кандидатів. P=PAR одночасних, ASCII-фільтр (скам),
# резюм (пропускає готові файли). Пінгає Telegram на завершенні.
set -uo pipefail
ROOT=/opt/wallet-tracker; ENVF=$ROOT/.env; ND=$ROOT/dataset/nextaction
LIST=$ND/candidates.tsv; OUT=$ND/prices; LOG=$ROOT/deploy/step-b-prices.log
SPAN=500; PAR=${PAR:-20}
mkdir -p "$OUT"; export OUT SPAN
ts(){ date -u +%Y-%m-%dT%H:%M:%SZ; }
get(){ grep -E "^$1=" "$ENVF" | head -1 | cut -d= -f2-; }
TOK=$(get TELEGRAM_BOT_TOKEN); CHAT=$(get TELEGRAM_CHAT_ID)
fetch_one(){
  local chain="$1" address="$2"
  [ -z "$address" ] && return 0
  local f="$OUT/${chain}_${address}.json"
  [ -s "$f" ] && return 0
  local lc="$chain"; case "$chain" in eth) lc=ethereum;; zksync) lc=era;; esac
  local key="${lc}:${address}" resp n
  resp=$(curl -s --max-time 25 "https://coins.llama.fi/chart/${key}?period=1d&span=${SPAN}")
  n=$(printf '%s' "$resp" | jq -r --arg k "$key" '.coins[$k].prices | length' 2>/dev/null)
  if [ -n "$n" ] && [ "$n" != "null" ] && [ "$n" -gt 0 ] 2>/dev/null; then
    printf '%s' "$resp" | jq -c --arg k "$key" '.coins[$k] | {symbol, decimals, prices}' > "$f" 2>/dev/null
  fi
  return 0
}
export -f fetch_one
echo "$(ts) Крок B FAST (P=$PAR, ASCII) старт; кандидатів після фільтра: $(awk -F"\t" '$1!="chain" && $2!="" && $3 ~ /^[ -~]+$/' "$LIST" | wc -l)" >> "$LOG"
awk -F'\t' '$1!="chain" && $2!="" && $3 ~ /^[ -~]+$/ {print $1, $2}' "$LIST" \
  | xargs -P "$PAR" -n 2 bash -c 'fetch_one "$1" "$2"' _
tot=$(ls "$OUT" | wc -l)
echo "$(ts) Крок B FAST готово: price files=$tot" >> "$LOG"
[ -n "$TOK" ] && [ -n "$CHAT" ] && curl -s --max-time 20 "https://api.telegram.org/bot$TOK/sendMessage" --data-urlencode "chat_id=$CHAT" --data-urlencode "text=Wallet Radar: Крок B (ціни) готовий - $tot price-файлів." >/dev/null
