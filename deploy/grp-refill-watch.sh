#!/usr/bin/env bash
# Сторож докачки групи (backfill2, список grp-refill-wallets.txt, ланцюг eth).
#
# Навіщо: докачка йде багато годин, а падіння процесу чи перезавантаження ВМ інакше
# зупинили б її мовчки. Стан лежить у _progress-grp.csv, тож рестарт продовжує з місця,
# а не починає спочатку. Робочий transfers.csv не чіпається — пишемо в окремий файл.
set -uo pipefail
ROOT=/opt/wallet-tracker
B=$ROOT/dataset/backfill
BF=$ROOT/backfill
LOG=$ROOT/deploy/grp-refill-watch.log
RUNLOG=$BF/run-grp.log
ST_DONE=$ROOT/deploy/.grp-refill-done
LIST=$B/grp-refill-wallets.txt
TS=$(date -u +%Y-%m-%dT%H:%M:%SZ)
get(){ grep -E "^$1=" "$ROOT/.env" | head -1 | cut -d= -f2-; }
send(){ local t; t=$(get TELEGRAM_BOT_TOKEN); local c; c=$(get TELEGRAM_CHAT_ID)
  [ -z "$t" ] && return
  curl -s --max-time 20 "https://api.telegram.org/bot$t/sendMessage" \
    --data-urlencode "chat_id=$c" --data-urlencode "text=$1" -d parse_mode=HTML >/dev/null; }

running=0; pgrep -x backfill2 >/dev/null 2>&1 && running=1
done_pairs=$(grep -c ",done," "$B/_progress-grp.csv" 2>/dev/null || echo 0)
total=$(wc -l < "$LIST" 2>/dev/null || echo 0)
# transfers-grp.csv — 105+ ГБ. Звичайний "wc -l" протягував його весь через
# сторінковий кеш кожні 15 хвилин, вимивав RAM у своп і роняв ядро в soft
# lockup (аварія 24.08.2026: Comm: wc, RIP: filemap_get_read_batch).
# fastcount.sh дочитує лише приріст — 0.02 с замість 3.7 хвилини.
rows=$("$ROOT/deploy/fastcount.sh" "$B/transfers-grp.csv" 2>/dev/null || echo 0)

# Скільки гаманців зі списку НЕ мають жодного успішного збору. Саме це число, а не
# «прогін дійшов до кінця», є мірою завершеності: гаманець, що впав на ліміті Etherscan,
# лишає по собі рядок err, і прогін спокійно йде далі.
dtmp=$(mktemp); ltmp=$(mktemp)
awk -F, '$4=="done"{print tolower($1)}' "$B/_progress-grp.csv" 2>/dev/null | sort -u > "$dtmp"
tr '[:upper:]' '[:lower:]' < "$LIST" 2>/dev/null | sort -u > "$ltmp"
left=$(comm -23 "$ltmp" "$dtmp" | wc -l)
rm -f "$dtmp" "$ltmp"
echo "$TS running=$running done=$done_pairs/$total left=$left rows=$rows" >> "$LOG"

# Прогін дійшов до кінця списку, якщо "complete" стоїть після останнього хедера запуску.
# АЛЕ це ще не завершення роботи: 14.08 прогін закрився, лишивши 5032 гаманці, що впали
# на добовій квоті Etherscan, і сторож надіслав "ГОТОВО" при 95.8% зібраного. Відмова через
# ліміт виглядає як завершення — тому рішення ухвалюємо за ЗАЛИШКОМ, а не за фактом кінця.
ST_LEFT=$ROOT/deploy/.grp-refill-left
ST_STALL=$ROOT/deploy/.grp-refill-stall
MAX_STALL=${MAX_STALL:-2}
hdr=$(grep -n 'wallets x' "$RUNLOG" 2>/dev/null | tail -1 | cut -d: -f1)
comp=$(grep -n '\[backfill\] complete' "$RUNLOG" 2>/dev/null | tail -1 | cut -d: -f1)
if [ -n "${comp:-}" ] && [ -n "${hdr:-}" ] && [ "$comp" -gt "$hdr" ]; then
  [ -f "$ST_DONE" ] && exit 0

  if [ "$left" -eq 0 ]; then
    touch "$ST_DONE"
    send "$(printf '\xF0\x9F\x8F\x81 <b>Wallet Radar</b>: докачка групи ГОТОВА — усі %s гаманців зібрано, %s рядків у transfers-grp.csv. Можна перераховувати cexemis.' "$total" "$rows")"
    exit 0
  fi

  # Залишок є. Якщо він тане — пускаємо ще прохід (backfill2 резюмиться й бере лише
  # незібране). Якщо не тане MAX_STALL проходів поспіль, далі товкти квоту марно:
  # це вже не ліміт, а адреси, яких Etherscan не віддає в принципі.
  # ВИЧЕРПАНА КВОТА — це не застій. 19.08 прохід закінчився з 477 відмовами
  # "Max daily rate limit reached", залишок не зменшився, лічильник застою дійшов до
  # межі — і сторож зупинив докачку НАЗАВЖДИ, хоча квота скидається щодоби о 00:10 UTC.
  # Тому: якщо останній прохід упирався в добовий ліміт, чекаємо скидання й пробуємо знову,
  # а застій не рахуємо.
  if tail -3000 "$RUNLOG" 2>/dev/null | grep -q "Max daily rate limit"; then
    # скидання квоти — 00:10 UTC; лог, старіший за сьогоднішнє скидання, означає що квота вже нова
    reset_ts=$(date -u -d "today 00:10" +%s)
    log_ts=$(stat -c %Y "$RUNLOG" 2>/dev/null || echo 0)
    if [ "$log_ts" -lt "$reset_ts" ]; then
      echo "$TS квота вже скинулась — новий прохід (лишилось $left)" >> "$LOG"
    else
      echo "$TS добова квота вичерпана, чекаємо скидання о 00:10 UTC (лишилось $left)" >> "$LOG"
      exit 0
    fi
  fi

  prev=$(cat "$ST_LEFT" 2>/dev/null || echo -1)
  stall=$(cat "$ST_STALL" 2>/dev/null || echo 0)
  if [ "$prev" -ge 0 ] && [ "$left" -ge "$prev" ]; then stall=$((stall + 1)); else stall=0; fi
  echo "$left" > "$ST_LEFT"; echo "$stall" > "$ST_STALL"
  echo "$TS прохід завершено, незібраних $left (було $prev), застій $stall" >> "$LOG"

  if [ "$stall" -ge "$MAX_STALL" ]; then
    touch "$ST_DONE"
    send "$(printf '\xE2\x9A\xA0 <b>Wallet Radar</b>: докачка групи СТАЛА — %s гаманців не зібрано за %s проходи поспіль (зі %s, рядків %s). Повтори вже не помагають, треба дивитись причину.' "$left" "$MAX_STALL" "$total" "$rows")"
    exit 0
  fi
  send "$(printf '\xF0\x9F\x94\x81 <b>Wallet Radar</b>: прохід докачки завершено, лишилось %s гаманців зі %s — пускаю ще один.' "$left" "$total")"
fi
[ "$running" -eq 1 ] && exit 0

# ── дві умови ПЕРЕД запуском, обидві вистраждані ──────────────────────────────
# 1) Гонка. Крон і ручний запуск за хвилину один від одного дали ДВА краули: обидва
#    бачили running=0, бо процес ще не встиг зʼявитись. Перевіряємо ще раз упритул.
sleep $((RANDOM % 5))
pgrep -x backfill2 >/dev/null 2>&1 && exit 0

# 2) Вичерпана квота. Поки добовий ліміт вибрано, запуск лише палить ретраї: краул
#    крутиться, пише err на кожен гаманець і нічого не збирає. Ліміт скидається о 00:10
#    UTC, тож чекаємо саме до цього моменту. Перевірка стоїть ТУТ, а не в гілці
#    «прохід завершено»: новий запуск дописує заголовок, гілка перестає спрацьовувати,
#    і перевірка обходилась стороною.
if tail -300 "$RUNLOG" 2>/dev/null | grep -q "Max daily rate limit"; then
  reset_ts=$(date -u -d "today 00:10" +%s)
  log_ts=$(stat -c %Y "$RUNLOG" 2>/dev/null || echo 0)
  if [ "$log_ts" -ge "$reset_ts" ]; then
    echo "$TS добова квота вичерпана, чекаємо скидання о 00:10 UTC (лишилось ${left:-?})" >> "$LOG"
    exit 0
  fi
fi

# Не працює і не завершено -> підняти з чекпойнту тими самими параметрами.
cd "$BF" || exit 1
K=$(grep -E "^BACKFILL_ETHERSCAN_KEYS=" "$ROOT/deploy/backfill.env" | head -1 | cut -d= -f2-)
APP_ROOT=$ROOT ETHERSCAN_API_KEYS="$K" CONCURRENCY=32 CHAINS=eth \
  WALLETS_FILE=$LIST OUT_FILE=$B/transfers-grp.csv PROG_FILE=$B/_progress-grp.csv \
  setsid ./target/release/backfill2 >> "$RUNLOG" 2>&1 < /dev/null &
echo "$TS restarted" >> "$LOG"
