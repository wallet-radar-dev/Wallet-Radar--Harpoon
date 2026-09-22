#!/usr/bin/env bash
# Послідовний збір нативного ETH: NET -> NET-WEAK.
#
# Послідовно, а не паралельно: квота Etherscan спільна (8 ключів x 3 запити/с), і два
# збирачі просто відбирали б її один в одного, множачи відмови замість швидкості.
#
# Після кожного проходу рахуємо ЗАЛИШОК — гаманці, у яких не завершено обидва види
# (ext і int). Прохід, що дійшов до кінця списку, ще не означає, що все зібрано:
# гаманець, який упав на ліміті, лишає статус err і мовчки лишається недокачаним.
# Спроб обмежено: якщо залишок перестав танути, далі товкти квоту марно.
set -uo pipefail
ROOT=/opt/wallet-tracker
B=$ROOT/dataset/backfill
LOG=/opt/wallet-tracker/deploy/ethflow-runner.log
cd "$ROOT/backfill" || exit 1

# PID-файл, а не пошук за іменем: pgrep -f ловить будь-який рядок, де згадано скрипт
# (навіть чужу ssh-команду), і сторож через це тричі бачив неіснуючу роботу.
PIDF=$ROOT/deploy/.ethflow-runner.pid
echo $$ > "$PIDF"
trap 'rm -f "$PIDF"' EXIT

left_of() {  # $1 = файл зі списком гаманців
  comm -23 <(sort -u "$1") \
    <(awk -F, '$4=="done"{print tolower($1)"|"$3}' "$B/_progress-native.csv" 2>/dev/null \
      | sort -u | awk -F'|' '{c[$1]++} END{for(w in c) if(c[w]>=2) print w}' | sort) | wc -l
}

# не заважаємо збору, який уже йде (HUB)
while pgrep -x ethflow >/dev/null 2>&1; do sleep 30; done

for G in HUB NET NET-WEAK; do
  LIST=$B/ethflow-wallets-$G.txt
  [ -f "$LIST" ] || { echo "$(date -u +%FT%TZ) нема $LIST" >> "$LOG"; continue; }
  prev=-1
  for try in 1 2 3 4 5; do
    echo "$(date -u +%FT%TZ) $G прохід $try, лишилось $(left_of "$LIST")" >> "$LOG"
    WALLETS_FILE="$LIST" OUT_FILE="$B/transfers-native.csv" PROG_FILE="$B/_progress-native.csv" \
      CONCURRENCY=12 ./target/release/ethflow "$ROOT" >> "/opt/wallet-tracker/deploy/ethflow-$G.log" 2>&1
    left=$(left_of "$LIST")
    echo "$(date -u +%FT%TZ) $G прохід $try завершено, лишилось $left" >> "$LOG"
    [ "$left" -eq 0 ] && break
    if [ "$prev" -ge 0 ] && [ "$left" -ge "$prev" ]; then
      echo "$(date -u +%FT%TZ) $G залишок не тане ($left) — зупиняюсь" >> "$LOG"
      break
    fi
    prev=$left
  done
done
echo "$(date -u +%FT%TZ) РУННЕР ЗАВЕРШИВ" >> "$LOG"
