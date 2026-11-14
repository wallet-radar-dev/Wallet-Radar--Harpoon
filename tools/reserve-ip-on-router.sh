#!/bin/bash
# DHCP-резервація адреси Forgejo-хоста на Tenda-роутері.
#
# Навіщо: SSH_DOMAIN у /etc/forgejo/app.ini вказує IP ноутбука, а IP
# видається по DHCP і вже змінювався (.112 → .104). Резервація прив'язує
# адресу до MAC — тож після будь-якого перезавантаження ноутбук знову
# отримає ту саму адресу, і clone-URL у Forgejo лишатиметься правильним.
#
# Роутер: Tenda, веб-інтерфейс на http://192.168.2.1, модуль «Advanced» →
# «Static IP List» (внутрішньо: goform/setNAT, поле staticList).
#
# Формат значення staticList: "<IP>\t<MAC>\t<REMARK>" — рядки розділені \n.
#
#   ./tools/reserve-ip-on-router.sh
#
# Ідемпотентний: якщо запис уже є — нічого не змінює.
set -euo pipefail

ROUTER="${ROUTER:-192.168.2.1}"
IP="${IP:-192.168.2.104}"
MAC="${MAC:-$(ip link show "$(ip route get 1.1.1.1 | awk '{print $5; exit}')" \
    | grep -oE 'link/ether [0-9a-f:]+' | cut -d' ' -f2 | tr 'a-f' 'A-F')}"
REMARK="${REMARK:-forgejo-host}"
COOKIES="$(mktemp)"
trap 'rm -f "$COOKIES"' EXIT

log() { echo "[$(date '+%Y-%m-%d %H:%M:%S')] $*"; }

if [ -z "$MAC" ]; then
    echo "не вдалося визначити MAC свого інтерфейсу" >&2
    exit 1
fi

log "роутер=${ROUTER} ip=${IP} mac=${MAC}"

# Чи вже є такий запис?
current=$(curl -s -m 10 -b "$COOKIES" \
    "http://${ROUTER}/goform/getNAT?modules=staticIPList")
if echo "$current" | grep -qi "$MAC"; then
    log "резервація для ${MAC} уже є — нічого не змінюю"
    echo "$current"
    exit 0
fi

# Формат: IP \t MAC \t REMARK. Рядки — через \n.
payload=$(printf 'module1=staticIPList&staticList=%s\t%s\t%s\n' "$IP" "$MAC" "$REMARK")
resp=$(printf '%s' "$payload" | curl -s -m 15 -b "$COOKIES" \
    -X POST --data-binary @- "http://${ROUTER}/goform/setNAT")

if ! echo "$resp" | grep -q '"errCode":"0"'; then
    log "роутер відхилив запис: $resp" >&2
    exit 1
fi
log "запис прийнято роутером"

# Перевірка: сервер має віддати саме наш запис.
after=$(curl -s -m 10 -b "$COOKIES" "http://${ROUTER}/goform/getNAT?modules=staticIPList")
if ! echo "$after" | grep -qi "$MAC"; then
    log "перевірка не пройшла: $after" >&2
    exit 1
fi
log "підтверджено роутером:"
echo "$after"
