#!/bin/bash
# Наглядач: чекає, поки ВМ 192.168.2.222 підніметься, і один раз запускає
# tools/fix-remote-after-ip-change.sh — щоб remote Wallet Radar вказував на
# актуальну адресу Forgejo (адреса ноутбука змінилась: .112 → .104).
#
# Журнал: /var/log/wra-fix-remote.log (або stdout systemd-журналу).
#
# Ідемпотентний: маркер-файл не дає зробити роботу двічі.
set -uo pipefail

VM_HOST="${VM_HOST:-192.168.2.222}"
SCRIPT="${SCRIPT:-/root/src/wallet-radar-alerts-bot/tools/fix-remote-after-ip-change.sh}"
MARKER="${MARKER:-/var/lib/wra-fix-remote.done}"
TIMEOUT_S="${TIMEOUT_S:-15}"

log() { echo "[$(date '+%Y-%m-%d %H:%M:%S')] $*"; }

if [ -f "$MARKER" ]; then
    log "уже виконано (маркер ${MARKER}) — нічого не роблю"
    exit 0
fi

# Перевіряємо не ping, а SSH: ping проходить, а сервіс — ні.
if ! timeout "$TIMEOUT_S" ssh -o BatchMode=yes -o StrictHostKeyChecking=no \
    -o ConnectTimeout=6 "root@${VM_HOST}" true 2>/dev/null; then
    # Код 0: ВМ просто ще не піднялася — це не помилка, а нормальний
    # «ще чекаємо». Ненульовий код тут засмічував би журнал помилками
    # systemd на кожному такті.
    log "${VM_HOST} ще недоступна за SSH — чекаю далі"
    exit 0
fi

log "${VM_HOST} доступна за SSH — запускаю ${SCRIPT}"
if bash "$SCRIPT"; then
    mkdir -p "$(dirname "$MARKER")"
    printf 'done at %s\n' "$(date -Is)" >"$MARKER"
    log "готово: remote оновлено"
    exit 0
fi

log "скрипт завершився з помилкою — спробую ще раз на наступному такті"
exit 1
