#!/usr/bin/env bash
# Перерахунок булрану-2021 на повних даних (358 млн рядків проти 202 млн, на яких рахувалось).
# Порядок кроків не довільний: спершу СКАН знаходить, де групи сиділи (цін не потребує),
# і лише потім добираються емісія та ціни саме на ці активи. Якщо почати з наявних цін,
# всесвіт звузиться до випадково викачаного, і у відповідь потраплять стейбли.
set -uo pipefail
ROOT=/opt/wallet-tracker
B=$ROOT/dataset/backfill
LOG=$ROOT/deploy/bullsell-rerun.log
cd "$ROOT/backfill" || exit 1
say() { echo "$(date -u +%Y-%m-%dT%H:%M:%SZ) $*" >> "$LOG"; }

say "1/5 скан: де сиділи групи"
MODE=scan ONLY_DONE=0 MIN_SHARE=5 TAG=2021all TOP=600 \
  ./target/release/bullsell "$ROOT" >> "$LOG" 2>&1

say "2/5 емісія для нових активів (без FORCE — відоме не перепитуємо)"
TOKENS_FILE=$B/bullsell-tokens-2021all.txt SLEEP_MS=400 \
  ./target/release/toksupply "$ROOT" >> "$LOG" 2>&1

say "3/5 денні ціни для нових активів (файл ЗЛИВАЄТЬСЯ, не перезаписується)"
TOKENS_FILE=$B/bullsell-tokens-2021all.txt SPAN=2100 BATCH=1 SLEEP_MS=350 \
  ./target/release/histprices "$ROOT" >> "$LOG" 2>&1

say "4/5 дати розворотів -> емісія на ті дати"
MODE=full ONLY_DONE=0 MIN_SHARE=1 TAG=2021pk \
  ./target/release/bullsell "$ROOT" >> "$LOG" 2>&1
PAIRS_FILE=$B/bullsell-peaks-2021pk.txt SLEEP_MS=300 \
  ./target/release/supplyat "$ROOT" >> "$LOG" 2>&1

say "5/5 фінал на знаменниках із тієї епохи"
MODE=full ONLY_DONE=0 MIN_SHARE=1 TAG=2021final \
  ./target/release/bullsell "$ROOT" >> "$LOG" 2>&1

say "ГОТОВО"
