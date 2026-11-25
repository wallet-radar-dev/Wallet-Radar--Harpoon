#!/bin/bash
# Оновлення remote у Wallet Radar на Forgejo: адреса SSH-сервера змінилась
# (ноутбук тепер 192.168.2.104, Forgejo слухає SSH_DOMAIN з app.ini).
#
# Користуватися, коли ВМ 192.168.2.222 доступна за SSH.
#
#   ./tools/fix-remote-after-ip-change.sh
#
# Скрипт ідемпотентний: повторний запуск просто перезапише ту саму адресу.
#
# Важливо: git на ВМ працює від користувача `ubuntu` — саме в нього є
# приватний ключ `~/.ssh/wallet_tracker_gitea` і конфіг ssh для Forgejo.
# У root'а на ВМ ключів для Forgejo немає, тому попередня версія скрипта
# падала з «Permission denied (publickey)» / «Host key verification failed».
set -euo pipefail

VM_HOST="${VM_HOST:-192.168.2.222}"
VM_USER="${VM_USER:-ubuntu}"
OLD_SSH_HOST="${OLD_SSH_HOST:-192.168.2.112}"
NEW_SSH_HOST="${NEW_SSH_HOST:-192.168.2.104}"
REPO_DIR="${REPO_DIR:-/opt/wallet-tracker}"
REPO_PATH="vitalii/wallet-tracker.git"
SSH_PORT="${SSH_PORT:-2222}"
KEY_NAME="${KEY_NAME:-wallet_tracker_gitea}"

ssh -o BatchMode=yes -o StrictHostKeyChecking=no "${VM_USER}@${VM_HOST}" bash -s <<EOF
set -euo pipefail
REPO_DIR="${REPO_DIR}"
REPO_PATH="${REPO_PATH}"
OLD_SSH_HOST="${OLD_SSH_HOST}"
NEW_SSH_HOST="${NEW_SSH_HOST}"
SSH_PORT="${SSH_PORT}"
KEY_NAME="${KEY_NAME}"

cd "\$REPO_DIR"

# 1. Ключ для Forgejo має бути саме у цього користувача.
if [ ! -f "\$HOME/.ssh/\${KEY_NAME}" ]; then
    echo "ПОМИЛКА: немає ключа \$HOME/.ssh/\${KEY_NAME}" >&2
    exit 1
fi
chmod 600 "\$HOME/.ssh/\${KEY_NAME}"

# 2. Конфіг ssh: окремий блок для актуальної адреси Forgejo + виправлення
#    старого блоку-аліаса, у якому залишилася стара адреса.
mkdir -p "\$HOME/.ssh"
chmod 700 "\$HOME/.ssh"
if ! grep -q "Host ${NEW_SSH_HOST}" "\$HOME/.ssh/config" 2>/dev/null; then
    cat >>"\$HOME/.ssh/config" <<CFG

Host ${NEW_SSH_HOST}
    HostName ${NEW_SSH_HOST}
    Port \${SSH_PORT}
    User git
    IdentityFile ~/.ssh/\${KEY_NAME}
    IdentitiesOnly yes
CFG
    echo "конфіг ssh: додано блок для ${NEW_SSH_HOST}"
fi
if grep -q "\${OLD_SSH_HOST}" "\$HOME/.ssh/config" 2>/dev/null; then
    sed -i "s/\${OLD_SSH_HOST}/\${NEW_SSH_HOST}/g" "\$HOME/.ssh/config"
    echo "конфіг ssh: замінив стару адресу \${OLD_SSH_HOST} на \${NEW_SSH_HOST}"
fi

# 3. Remote у git: стара адреса означає, що forgejo на гілці недоступний:
#    гілки оновлювати неможливо, а код «з'їжджає» з машини.
if git remote get-url origin | grep -q "\${OLD_SSH_HOST}"; then
    echo "remote указує на застарілу адресу \${OLD_SSH_HOST} — оновлюю"
    git remote set-url origin "ssh://git@\${NEW_SSH_HOST}:\${SSH_PORT}/\${REPO_PATH}"
else
    echo "remote уже актуальний — лишаю як є"
    git remote set-url origin "ssh://git@\${NEW_SSH_HOST}:\${SSH_PORT}/\${REPO_PATH}"
fi

echo "--- remote ---"
git remote -v

# 4. Перевірка: Forgejo має віддавати гілки саме з цієї машини.
#    accept-new додає host key у known_hosts, якщо його там ще немає.
echo "--- перевірка доступу до Forgejo з цієї ВМ ---"
export GIT_SSH_COMMAND="ssh -i \$HOME/.ssh/\${KEY_NAME} -o IdentitiesOnly=yes -o StrictHostKeyChecking=accept-new"
if git ls-remote --exit-code origin >/dev/null 2>&1; then
    echo "OK: Forgejo віддає гілки, доступ працює"
    git ls-remote --heads origin | head -5
else
    echo "ПОМИЛКА: Forgejo недоступний з цієї ВМ" >&2
    echo "Перевір: ключ ~/.ssh/\${KEY_NAME} і SSH_DOMAIN у /etc/forgejo/app.ini на сервері Forgejo" >&2
    exit 1
fi
EOF
