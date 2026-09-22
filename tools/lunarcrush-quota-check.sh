#!/usr/bin/env bash
# Знімок стану добової квоти LunarCrush одразу після її скидання (00:00 UTC).
#
# Навіщо окремий скрипт, а не «спитаю Claude вранці»: числа тут ЧАСОЗАЛЕЖНІ. Лічильники
# сателітів живуть у логах контейнерів, а вікно «перші хвилини після скидання» показує рівно те,
# що нас цікавить — чи почала система витрачати квоту заново і з якою швидкістю. Через шість
# годин цього вже не побачиш.
#
# Питання, на яке відповідає звіт: чи вписалися ми в 100 викликів/добу після фіксів
# (соц-кеш 12 год на диску + кеш aflow, який накрив увесь розрахунок).
set -uo pipefail
cd "$(dirname "$0")/.."

OUT="dataset/.quota-report-$(date -u +%Y%m%d-%H%M).txt"
exec > >(tee "$OUT") 2>&1

echo "════ ЗВІТ ПРО КВОТУ LunarCrush ════"
echo "знято: $(date -u '+%Y-%m-%d %H:%M UTC')  /  $(date '+%H:%M %Z')"
echo

# ── 1. Що каже сам провайдер ──
KEY=$(grep -E "^SAT_A_LUNARCRUSH_API_KEY=" deploy/.env 2>/dev/null | cut -d= -f2-)
if [[ -z "$KEY" ]]; then
  echo "✗ ключа LunarCrush немає в deploy/.env — перевіряти нічого"
else
  echo "── Квота за словами провайдера ──"
  curl -s -D - -o /dev/null -H "Authorization: Bearer $KEY" \
    "https://lunarcrush.com/api4/public/coins/BTC/v1" 2>/dev/null \
    | grep -iE "^x-rate-limit|^HTTP" | sed 's/\r$//;s/^/  /'
fi
echo

# ── 2. Скільки ми витратили ВІД СКИДАННЯ КВОТИ ──
#
# Рахувати «за останні 24 год» тут НЕ МОЖНА, і це не дрібниця. Квота скидається о 00:00 UTC, тож
# одразу після скидання у 24-годинне вікно потрапляє вся ПОПЕРЕДНЯ, спалена доба — і скрипт
# упевнено оголосив би «квота знову вичерпана», показуючи старі відмови. Рахуємо тільки нову добу.
SINCE_MIN=$(( $(date -u +%H) * 60 + $(date -u +%M) + 1 ))
HOURS_IN=$(awk "BEGIN{printf \"%.1f\", $SINCE_MIN/60}")
echo "── Виклики LunarCrush ВІД скидання квоти (${HOURS_IN} год нової доби) ──"
total=0; daily=0
for c in satellite-a satellite-b; do
  n=$(docker logs --since "${SINCE_MIN}m" "$c" 2>&1 | grep -c "lunarcrush /api4" || echo 0)
  d=$(docker logs --since "${SINCE_MIN}m" "$c" 2>&1 | grep -c "ЛІМІТ-ДОБОВИЙ" || echo 0)
  echo "  $c: викликів=$n, добових відмов=$d"
  total=$((total + n)); daily=$((daily + d))
done
echo "  РАЗОМ за нову добу: $total викликів, $daily відмов за добовим лімітом"
echo

# ── 3. Вердикт (залежить від того, скільки доби минуло) ──
echo "── Вердикт ──"
REMAIN=$(curl -s -D - -o /dev/null -H "Authorization: Bearer $KEY" \
  "https://lunarcrush.com/api4/public/coins/BTC/v1" 2>/dev/null \
  | grep -i "^x-rate-limit-day-remaining" | tr -dc '0-9-')

if [[ $SINCE_MIN -lt 120 ]]; then
  # Перші дві години нової доби: про БЮДЖЕТ судити ще рано — минуло надто мало часу.
  # Перевіряємо лише те, що в цей момент і можна перевірити: квота ожила, виклики проходять.
  echo "  Минуло лише ${HOURS_IN} год нової доби — про добовий бюджет судити ЗАРАНО."
  echo "  Зараз перевіряємо тільки одне: чи квота ожила і чи проходять виклики."
  if [[ -n "$REMAIN" && "$REMAIN" -gt 0 ]]; then
    echo "  ✓ квота скинулась: лишилось $REMAIN зі 100"
  else
    echo "  ✗ квота НЕ ожила (залишок: ${REMAIN:-?}) — або скидання не о 00:00 UTC, або ми вже все спалили"
  fi
  echo "  ⚠ ПОВНИЙ вердикт про бюджет дасть лише перевірка наприкінці доби (~23:30 UTC)."
elif [[ $daily -gt 0 ]]; then
  echo "  ✗ ДОБОВА КВОТА ЗНОВУ ВИЧЕРПАНА ($daily відмов за ${HOURS_IN} год)."
  echo "    Отже, 12-годинного TTL замало. Наступний крок: підняти SOCIAL_TTL у prices.rs"
  echo "    (24 год → ~30 викликів/добу) або звузити коло активів."
else
  rate=$(awk "BEGIN{d=$SINCE_MIN/1440; if(d<0.001)d=0.001; printf \"%.0f\", $total/d}")
  echo "  ✓ добових відмов немає. Витрата: $total за ${HOURS_IN} год → ~$rate на добу в такому темпі."
  if [[ "$rate" -le 100 ]]; then
    echo "    Вписуємось у бюджет 100/добу. Фікси спрацювали."
  else
    echo "    ⚠ у такому темпі вийде ~$rate > 100 — до кінця доби впремося в ліміт."
  fi
fi
echo

# ── 4. Соц-кеш: чи він реально працює ──
echo "── Соц-кеш на диску ──"
now=$(date +%s)
jq -r --argjson now "$now" '
  def pad($n): . + (" " * ([$n - length, 0] | max));
  "  символів: \(length)  (з даними: \([.[]|select(.[1])]|length), порожніх/відмов: \([.[]|select(.[1]|not)]|length))",
  (to_entries|sort_by(.key)[]| "    \(.key|pad(8)) \((if .value[1] then "дані" else "null" end)|pad(5)) вік \((($now-.value[0])/3600*10|round)/10) год")
' dataset/social-cache.json 2>/dev/null || echo "  (файлу ще немає — get_social ще не викликався)"
echo

# ── 5. Чи скаржився кор на деградацію ──
echo "── Кор: падіння на CoinGecko (соц-складова вимкнена) ──"
n=$(journalctl -u wallet-radar-rust.service --since "24 hours ago" --no-pager 2>/dev/null | grep -c "LunarCrush недоступний" || echo 0)
echo "  попереджень за добу: $n"
echo

# ── 6. Заразом: як там Etherscan ──
echo "── Etherscan (контроль, за 1 год) ──"
ec=0; el=0
for c in satellite-a satellite-b; do
  ec=$((ec + $(docker logs --since 1h "$c" 2>&1 | grep -c "etherscan /v2" || echo 0)))
  el=$((el + $(docker logs --since 1h "$c" 2>&1 | grep -cE "ЛІМІТ$| ЛІМІТ " || echo 0)))
done
lost=$(journalctl -u wallet-radar-rust.service --since "1 hour ago" --no-pager 2>/dev/null | grep -c "усі 3 спроби" || echo 0)
echo "  викликів: $ec, відмов: $el, ВТРАЧЕНО запитів: $lost"
echo
echo "звіт збережено: $OUT"
