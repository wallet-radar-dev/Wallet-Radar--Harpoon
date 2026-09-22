#!/usr/bin/env bash
# Прокрутка великого transfers-CSV в .zst-архів.
#
# Порядок жорсткий і НЕ переставляється: зупинити збирача → стиснути → звірити
# кількість рядків → і лише при точному збігу обнулити оригінал. Якщо машина впаде
# посеред стиснення (а вона падає), оригінал лишається цілим, а .tmp просто викидається.
set -uo pipefail

F=${1:?потрібен шлях до csv}
LOCK=${2:?потрібен файл замка}
PROC=${3:?потрібна назва процесу-збирача}

B=$(dirname "$F")
n=1; while [ -e "$F.$(printf %03d $n).zst" ]; do n=$((n+1)); done
A="$F.$(printf %03d $n).zst"
say(){ echo "$(date +%T) $*"; }

exec 9>"$LOCK"
flock -w 120 9 || { say "НЕ ВЗЯВ ЗАМОК — виходжу"; exit 1; }
say "замок узято ($LOCK)"

if pgrep -x "$PROC" >/dev/null; then
  pkill -TERM -x "$PROC" && say "$PROC: SIGTERM"
  for i in $(seq 1 40); do pgrep -x "$PROC" >/dev/null || break; sleep 1; done
  pgrep -x "$PROC" >/dev/null && { say "$PROC не зупинився — ПЕРЕРИВАЮ"; exit 1; }
fi
say "$PROC зупинено"

N=$(/opt/wallet-tracker/deploy/fastcount.sh "$F"); say "рядків в оригіналі: $N"
SZ=$(stat -c %s "$F");                            say "розмір: $SZ"
[ "$N" -gt 0 ] || { say "оригінал порожній — нема чого крутити"; exit 0; }

say "стискаю в $A.tmp …"
zstd -3 -T4 -q "$F" -o "$A.tmp" || { say "СТИСНЕННЯ ВПАЛО"; rm -f "$A.tmp"; exit 1; }
say "архів: $(stat -c %s "$A.tmp") байт"

say "звіряю рядки в архіві…"
M=$(zstd -dc "$A.tmp" | wc -l); say "рядків в архіві: $M"
if [ "$N" != "$M" ]; then say ">>> РОЗБІЖНІСТЬ $N vs $M — оригінал НЕ чіпаю"; exit 1; fi

mv "$A.tmp" "$A"
: > "$F"
say ">>> ГОТОВО. Архів $A, оригінал обнулено."
df -h "$B" | tail -1
