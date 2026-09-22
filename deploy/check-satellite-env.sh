#!/usr/bin/env bash
# Перевіряє deploy/.env перед підняттям сателітів. Значень НЕ друкує — лише довжини й вердикт.
#
# Ловить дві речі, які інакше помічаєш надто пізно:
#   1) ключ, що просочився назад у .env кора — це мовчки скасовує всю ізоляцію;
#   2) однакові ключі в A і B — квота НЕ множиться, хоча виглядає, ніби сателітів двоє.
set -uo pipefail
cd "$(dirname "$0")"

ENV_FILE=".env"
CORE_ENV="../.env"
PROVIDERS=(ETHERSCAN MORALIS LUNARCRUSH ARKHAM GLASSNODE)
rc=0

if [[ ! -f "$ENV_FILE" ]]; then
  echo "✗ немає deploy/.env — скопіюйте deploy/.env.example і заповніть"
  exit 1
fi

get() { grep -E "^$1=" "$2" 2>/dev/null | head -1 | cut -d= -f2-; }

echo "── Ключі в кора бути НЕ повинно"
leaked=0
for p in "${PROVIDERS[@]}"; do
  v=$(get "${p}_API_KEY" "$CORE_ENV")
  if [[ -n "$v" ]]; then
    echo "  ✗ ${p}_API_KEY знову лежить у .env кора — ізоляція скасована"
    leaked=1
    rc=1
  fi
done
[[ $leaked -eq 0 ]] && echo "  ✓ жодного ключа провайдера в .env кора немає"

echo
echo "── Сателіти"
for p in "${PROVIDERS[@]}"; do
  a=$(get "SAT_A_${p}_API_KEY" "$ENV_FILE")
  b=$(get "SAT_B_${p}_API_KEY" "$ENV_FILE")
  if [[ -z "$a" && -z "$b" ]]; then
    echo "  ∅ $p — ключа немає в жодного (Fetch віддасть виразну помилку, не тишу)"
  elif [[ -n "$a" && "$a" == "$b" ]]; then
    # Не помилка, а втрачена вигода — тому попередження, а не провал.
    echo "  ⚠ $p — A і B мають ОДНАКОВИЙ ключ: квота НЕ подвоюється, лише другий IP і резерв"
  elif [[ -z "$a" || -z "$b" ]]; then
    echo "  ~ $p — ключ лише в одного з двох (${#a}/${#b} символів): запити піде обслуговувати він один"
  else
    echo "  ✓ $p — різні ключі в A і B (${#a}/${#b} символів): квота множиться"
  fi
done

echo
[[ $rc -eq 0 ]] && echo "Готово до старту." || echo "Є проблеми — див. ✗ вище."
exit $rc
