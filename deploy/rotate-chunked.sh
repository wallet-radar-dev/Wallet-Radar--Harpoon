#!/usr/bin/env bash
# Прокрутка великого CSV у КІЛЬКА .zst-архівів.
#
# Навіщо частинами: 28.08 одноразове стиснення 143 GB дало архів, що не збігся зі
# своєю ж контрольною сумою (бракувало 423 рядків) — при нульових помилках ядра й
# незмінному джерелі. Байти псуються тихо, вже в памʼяті. Дрібніші частини локалізують
# шкоду: перездається одна частина, а не весь файл, і кожна має власну контрольну суму.
#
# Джерело чіпаємо ЛИШЕ якщо сума рядків усіх частин точно дорівнює оригіналу.
set -uo pipefail

F=${1:?потрібен шлях до csv}
LOCK=${2:?потрібен файл замка}
PROC=${3:?потрібна назва процесу-збирача}
PARTS=${PARTS:-8}
say(){ echo "$(date +%T) $*"; }

exec 9>"$LOCK"
flock -w 120 9 || { say "НЕ ВЗЯВ ЗАМОК"; exit 1; }
say "замок узято"

if pgrep -x "$PROC" >/dev/null; then
  pkill -TERM -x "$PROC"; say "$PROC: SIGTERM"
  for i in $(seq 1 40); do pgrep -x "$PROC" >/dev/null || break; sleep 1; done
  pgrep -x "$PROC" >/dev/null && { say "$PROC не зупинився — ПЕРЕРИВАЮ"; exit 1; }
fi
say "$PROC зупинено"

N=$(/opt/wallet-tracker/deploy/fastcount.sh "$F"); say "рядків в оригіналі: $N"
[ "$N" -gt 0 ] || { say "порожній — нема чого крутити"; exit 0; }

rm -f "$F".[0-9][0-9][0-9].zst.part
say "ріжу на $PARTS частин і стискаю..."
split -d -a 3 -n "l/$PARTS" --filter='zstd -3 -T2 -q -o "$FILE".zst.part' "$F" "$F." \
  || { say "SPLIT/ZSTD ВПАЛИ"; rm -f "$F".[0-9][0-9][0-9].zst.part; exit 1; }

say "звіряю кожну частину..."
tot=0; bad=0
for p in "$F".[0-9][0-9][0-9].zst.part; do
  if ! zstd -t "$p" >/dev/null 2>&1; then say "  ЧАСТИНА ПОБИТА: $p"; bad=$((bad+1)); continue; fi
  c=$(zstd -dc "$p" | wc -l); tot=$((tot+c))
  say "  $(basename "$p"): $c рядків, $(stat -c %s "$p") байт"
done

if [ "$bad" -ne 0 ] || [ "$tot" != "$N" ]; then
  say ">>> НЕ ЗІЙШЛОСЬ: сума $tot проти $N (побитих частин: $bad). Джерело НЕ чіпаю."
  exit 1
fi

for p in "$F".[0-9][0-9][0-9].zst.part; do mv "$p" "${p%.part}"; done
: > "$F"
say ">>> ГОТОВО. Сума рядків $tot = $N. Оригінал обнулено."
df -h "$(dirname "$F")" | tail -1
