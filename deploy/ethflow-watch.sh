#!/usr/bin/env bash
# Сторож збору нативного ETH: піднімає руннер, якщо той не працює, а робота лишилась.
#
# Навіщо: 18.08 ВМ перезавантажилась, і збір помер разом із нею — він жив як голий процес
# у setsid, а не як сервіс. Логи в /tmp зникли, черга NET/NET-WEAK так і не почалась.
# Усе, що не в systemd або крон-сторожі, після ребуту просто не існує.
#
# Ставиться в крон раз на 15 хв. Сам збирач резюмиться з _progress-native.csv, тож
# повторний запуск не перекачує зібране.
set -uo pipefail
ROOT=/opt/wallet-tracker
B=$ROOT/dataset/backfill
LOG=$ROOT/deploy/ethflow-watch.log
TS=$(date -u +%Y-%m-%dT%H:%M:%SZ)

# гаманці, де завершено ОБИДВА види (ext і int) — решта вважається недокачаною
done_both() {
  awk -F, '$4=="done"{print tolower($1)"|"$3}' "$B/_progress-native.csv" 2>/dev/null \
    | sort -u | awk -F'|' '{c[$1]++} END{for(w in c) if(c[w]>=2) print w}' | sort
}
left=0
for G in HUB NET NET-WEAK; do
  L=$B/ethflow-wallets-$G.txt
  [ -f "$L" ] || continue
  n=$(comm -23 <(sort -u "$L") <(done_both) | wc -l)
  left=$((left + n))
done
# transfers-native.csv — 26+ ГБ, більше за всю RAM машини. Звичайний `wc -l`
# кожні 15 хвилин вимивав увесь сторінковий кеш. Те саме лікування, що й у
# grp-refill-watch.sh: дочитуємо лише приріст.
rows=$(/opt/wallet-tracker/deploy/fastcount.sh "$B/transfers-native.csv" 2>/dev/null || echo 0)

running=0
pgrep -x ethflow >/dev/null 2>&1 && running=1
# Руннер — за PID-файлом, але з перевіркою, ЧИЙ це процес. Самого kill -0 замало:
# після вбитого руннера файл лишається, номер PID згодом дістається чужому процесу,
# і сторож вічно бачить неіснуючу роботу (20.08: збір стояв годинами при running=1).
PIDF=$ROOT/deploy/.ethflow-runner.pid
if [ -f "$PIDF" ]; then
  RP=$(cat "$PIDF" 2>/dev/null)
  if [ -n "$RP" ] && tr -d '\0' < "/proc/$RP/cmdline" 2>/dev/null | grep -q "ethflow-runner"; then
    running=1
  else
    rm -f "$PIDF"   # застарілий файл прибираємо, інакше він блокує сторожа назавжди
  fi
fi

echo "$TS running=$running left=$left rows=$rows" >> "$LOG"
[ "$left" -eq 0 ] && exit 0
[ "$running" -eq 1 ] && exit 0

setsid "$ROOT/deploy/ethflow-runner.sh" >/dev/null 2>&1 < /dev/null &
echo "$TS піднято руннер (лишилось $left)" >> "$LOG"
