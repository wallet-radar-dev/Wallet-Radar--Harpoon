#!/usr/bin/env bash
# Перевірка МУЛЬТИ-горизонтного forward-треку ML (1/7/14/30/365д).
# Пінгує Telegram ОКРЕМО на кожному горизонті, коли той уперше публікує AUC,
# з чесним вердиктом: сигнал (auc помітно > null) чи шум (auc ≈ null).
set -uo pipefail
ENVF=/opt/wallet-tracker/.env
DIR=/opt/wallet-tracker/deploy
LOG=$DIR/fwd-check.log
get(){ grep -E "^$1=" "$ENVF" | head -1 | cut -d= -f2-; }
PW=$(get APP_PASSWORD); TOK=$(get TELEGRAM_BOT_TOKEN); CHAT=$(get TELEGRAM_CHAT_ID)
TS=$(date -u +%Y-%m-%dT%H:%M:%SZ)

J=$(curl -s -u "app:$PW" --max-time 30 http://127.0.0.1:3000/api/ml-forward || echo "{}")
SAMPLES=$(echo "$J" | jq -r ".samples // 0")
ALLPNL=$(echo "$J" | jq -r ".allTimePnlUsd // 0")
MSTR=$(echo "$J" | jq -r '.horizons // {} | to_entries | map("\(.key):\(.value.matured)") | join(" ")' 2>/dev/null)
echo "$TS matured[$MSTR] samples=$SAMPLES allPnl=$ALLPNL" >> "$LOG"

send(){
  if [ -z "$TOK" ] || [ -z "$CHAT" ]; then echo "$TS [warn] no token/chat_id — telegram skip" >> "$LOG"; return; fi
  curl -s --max-time 20 "https://api.telegram.org/bot$TOK/sendMessage" \
    --data-urlencode "chat_id=$CHAT" --data-urlencode "text=$1" -d parse_mode=HTML >/dev/null \
    && echo "$TS [tg] sent" >> "$LOG"
}

# Кожен горизонт — свій одноразовий пінг, коли з'явиться його AUC.
for h in 1 7 14 30 365; do
  st="$DIR/.fwd-h$h"
  [ -f "$st" ] && continue
  auc=$(echo "$J" | jq -r ".horizons[\"$h\"].auc")
  [ "$auc" = "null" ] || [ -z "$auc" ] && continue
  nul=$(echo "$J" | jq -r ".horizons[\"$h\"].nullAuc // 0")
  pos=$(echo "$J" | jq -r ".horizons[\"$h\"].posRate // \"?\"")
  mat=$(echo "$J" | jq -r ".horizons[\"$h\"].matured // 0")
  # Вердикт: auc помітно вище null (>0.03) = сигнал; інакше шум.
  verdict=$(echo "$J" | jq -r ".horizons[\"$h\"] | if ((.auc // 0) - (.nullAuc // 0)) > 0.03 then \"\xE2\x9C\x85 Є СИГНАЛ\" else \"\xE2\x9A\xA0 шум (auc = null)\" end")
  send "$(printf '\xF0\x9F\x8E\xAF <b>Wallet Radar</b>: %sд forward-AUC опубліковано (%s визрілих).\nauc=%s  null=%s  pos=%s%%\n-> %s' "$h" "$mat" "$auc" "$nul" "$pos" "$verdict")"
  touch "$st"
done
