#!/usr/bin/env bash
# Стежить за backfill-краулом. Пінгує Telegram при завершенні (🏁) або краху (⚠️).
# Надійний сигнал завершення — рядок "[backfill] complete" ПІСЛЯ останнього хедера запуску
# (лічильник [N/total] включає skip-пари, але complete-рядок однозначний).
set -uo pipefail
ENVF=/opt/wallet-tracker/.env
DIR=/opt/wallet-tracker/deploy
B=/opt/wallet-tracker/dataset/backfill
LOG=$DIR/backfill-watch.log
ST_DONE=$DIR/.backfill-done
ST_CRASH=$DIR/.backfill-crash
RUNLOG=$B/run.log
get(){ grep -E "^$1=" "$ENVF" | head -1 | cut -d= -f2-; }
TOK=$(get TELEGRAM_BOT_TOKEN); CHAT=$(get TELEGRAM_CHAT_ID)
TS=$(date -u +%Y-%m-%dT%H:%M:%SZ)

counter=$(grep -oE '\[[0-9]+/[0-9]+\]' "$RUNLOG" 2>/dev/null | tail -1 | tr -d '[]')
hdr=$(grep -n 'wallets x' "$RUNLOG" 2>/dev/null | tail -1 | cut -d: -f1)
comp=$(grep -n '\[backfill\] complete' "$RUNLOG" 2>/dev/null | tail -1 | cut -d: -f1)
running=0; pgrep -x backfill >/dev/null 2>&1 && running=1

# завершено, якщо "complete" йде ПІСЛЯ останнього хедера запуску
done_flag=0
if [ -n "${comp:-}" ] && [ -n "${hdr:-}" ] && [ "$comp" -gt "$hdr" ]; then done_flag=1; fi

echo "$TS running=$running counter=${counter:-?} done_flag=$done_flag" >> "$LOG"

send(){
  if [ -z "$TOK" ] || [ -z "$CHAT" ]; then echo "$TS [warn] no tg" >> "$LOG"; return; fi
  curl -s --max-time 20 "https://api.telegram.org/bot$TOK/sendMessage" \
    --data-urlencode "chat_id=$CHAT" --data-urlencode "text=$1" -d parse_mode=HTML >/dev/null \
    && echo "$TS [tg] sent" >> "$LOG"
}

# 1) Завершено -> пінг + АВТО-СТАРТ Кроку B (один раз)
if [ "$done_flag" -eq 1 ]; then
  if [ ! -f "$ST_DONE" ]; then
    touch "$ST_DONE"   # спершу стемп: гарантія «один раз», навіть якщо нижче щось впаде
    rows=$(wc -l < "$B/transfers.csv" 2>/dev/null || echo '?')
    send "$(printf '\xF0\x9F\x8F\x81 <b>Wallet Radar</b>: backfill ГОТОВИЙ (%s рядків).\nАвтоматично запускаю Крок B (збір цін).' "$rows")"
    # Крок A на ПОВНОМУ наборі: дамп списку кандидатів -> Крок B (фетч цін) у фоні.
    ND=/opt/wallet-tracker/dataset/nextaction
    mkdir -p "$ND"
    /opt/wallet-tracker/nextaction/target/release/netflow /opt/wallet-tracker 40 "$ND/candidates.tsv" >> "$LOG" 2>&1
    setsid /opt/wallet-tracker/deploy/step-b-prices.sh >> "$DIR/step-b-prices.cron.log" 2>&1 < /dev/null &
    echo "$TS [step-b] запущено фетч цін" >> "$LOG"
  fi
  rm -f "$ST_CRASH"
  exit 0
fi

# 2) Ще працює
if [ "$running" -eq 1 ]; then
  rm -f "$ST_CRASH"   # живий — скинути крах-стемп, щоб майбутній крах пінгнув знову
  exit 0
fi

# 3) Не завершено і не працює -> впав/зупинено
if [ ! -f "$ST_CRASH" ]; then
  send "$(printf '\xE2\x9A\xA0\xEF\xB8\x8F <b>Wallet Radar</b>: backfill ЗУПИНИВСЯ (%s, не завершено).\nЙмовірно впав — перезапустити (резюмиться з чекпойнту).' "${counter:-?}")"
  touch "$ST_CRASH"
fi
