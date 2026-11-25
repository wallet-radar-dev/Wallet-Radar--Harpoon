#!/bin/bash
# Оновлення remote у Wallet Radar на Forgejo: адреса SSH-сервера змінилась
# (ноутбук тепер 192.168.2.104, Forgejo слухає SSH_DOMAIN з app.ini).
#
# Користуватися, коли ВМ 192.168.2.222 доступна за SSH.
#
#   ./tools/fix-remote-after-ip-change.sh
#
# Скрипт ідемпотентний: повторний запуск просто перезапише ту саму адресу.
set -euo pipefail

VM_HOST="${VM_HOST:-192.168.2.222}"
OLD_SSH_HOST="${OLD_SSH_HOST:-192.168.2.112}"
NEW_SSH_HOST="${NEW_SSH_HOST:-192.168.2.104}"
REPO_DIR="${REPO_DIR:-/opt/wallet-tracker}"
REPO_PATH="vitalii/wallet-tracker.git"

ssh -o BatchMode=yes -o StrictHostKeyChecking=no "root@${VM_HOST}" bash -s <<EOF
set -euo pipefail
cd ${REPO_DIR}

# Стара адреса в remote означає, що forgejo на гілці недоступний: гілки
# оновлювати неможливо, а код «з’їжджає» з машини.
if git remote get-url origin | grep -q "${OLD_SSH_HOST}"; then
    echo "remote указує на застарілу адресу ${OLD_SSH_HOST} — оновлюю"
    git remote set-url origin "ssh://git@${NEW_SSH_HOST}:2222/${REPO_PATH}"
else
    echo "remote уже актуальний — лишаю як є"
    git remote set-url origin "ssh://git@${NEW_SSH_HOST}:2222/${REPO_PATH}"
fi

echo "--- remote ---"
git remote -v

# Перевірка: Forgejo має віддавати гілки саме з цієї машини.
# Тут і жив сам ключ (deploy-ключ «wallet-radar-vm-deploykey»), тож це
# справжня перевірка доступу, а не лише зміна рядка в конфізі.
echo "--- перевірка доступу до Forgejo з ВМ ---"
if git ls-remote --exit-code origin >/dev/null 2>&1; then
    echo "OK: Forgejo віддає гілки, доступ працює"
    git ls-remote --heads origin | head -5
else
    echo "ПОМИЛКА: Forgejo недоступний з цієї ВМ" >&2
    echo "Перевір: SSH-ключ на ВМ і SSH_DOMAIN у /etc/forgejo/app.ini на сервері Forgejo" >&2
    exit 1
fi
EOF
