#!/bin/bash
# Оновлення remote у Wallet Radar на Forgejo: адреса SSH-сервера змінилась
# (ноутбук тепер 192.168.2.104, Forgejo слухає SSH_DOMAIN з app.ini).
#
# Користуватися, коли ВМ 192.168.2.222 доступна за SSH.
set -euo pipefail

VM_HOST="${VM_HOST:-192.168.2.222}"
OLD_SSH_HOST="${OLD_SSH_HOST:-192.168.2.112}"
NEW_SSH_HOST="${NEW_SSH_HOST:-192.168.2.104}"

ssh -o BatchMode=yes -o StrictHostKeyChecking=no "root@${VM_HOST}" bash -s <<EOF
set -euo pipefail
cd /opt/wallet-tracker
git remote set-url origin "ssh://git@${NEW_SSH_HOST}:2222/vitalii/wallet-tracker.git"
echo "remote оновлено:"
git remote -v
EOF
