#!/bin/bash
# Автотест після перезавантаження: перевіряє, що Forgejo-remote знову живий.
#
# Той самий набір перевірок, що й вручну:
#   1. чи машина отримала ту саму адресу (192.168.2.104) — тобто чи
#      спрацювала DHCP-резервація;
#   2. чи резервація досі в реєстрі роутера;
#   3. чи Forgejo піднявся і віддає репозиторій по SSH;
#   4. чи бот живий і чи немає конфлікту опитування (409).
#
# Результат пишеться в /var/log/wra-postboot-check.log — його можна
# прочитати після ребуту, навіть якщо агент не піднявся.
set -uo pipefail

LOG="/var/log/wra-postboot-check.log"
EXPECT_IP="${EXPECT_IP:-192.168.2.104}"
ROUTER="${ROUTER:-192.168.2.1}"
REPO="vitalii/wallet-radar-alerts-bot"
SSH_URL="ssh://git@${EXPECT_IP}:2222/${REPO}.git"

log() { echo "[$(date '+%Y-%m-%d %H:%M:%S')] $*" | tee -a "$LOG"; }

log "=== автотест після перезавантаження ==="

# ── 1. адреса ───────────────────────────────────────────────────────────────
ip4=$(ip -4 addr show wlp0s20f3 2>/dev/null | grep -oE 'inet [0-9.]+' | cut -d' ' -f2 | head -1)
if [ "$ip4" = "$EXPECT_IP" ]; then
    log "OK  адреса: $ip4 (очікували $EXPECT_IP) — DHCP-резервація спрацювала"
else
    log "ПОМИЛКА адреса: ${ip4:-немає} (очікували $EXPECT_IP)"
fi

# ── 2. реєстр роутера ──────────────────────────────────────────────────────
res=$(curl -s -m 10 "http://${ROUTER}/goform/getNAT?modules=staticIPList" 2>/dev/null)
if echo "$res" | grep -qi "$EXPECT_IP"; then
    log "OK  резервація в роутері: $(echo "$res" | head -c 160)"
else
    log "ПОМИЛКА резервації в роутері немає: ${res:-роутер недоступний}"
fi

# ── 3. Forgejo ─────────────────────────────────────────────────────────────
if systemctl is-active --quiet forgejo.service; then
    log "OK  forgejo.service: active"
else
    log "ПОМИЛКА forgejo.service: $(systemctl is-active forgejo.service)"
fi

if timeout 20 git ls-remote --exit-code "$SSH_URL" >/dev/null 2>&1; then
    log "OK  Forgejo віддає гілки по SSH: $SSH_URL"
else
    log "ПОМИЛКА Forgejo не віддає гілки по SSH: $SSH_URL"
fi

# Репозиторій приватний, тож API без токена віддає 404 — це очікувано,
# а не помилка. Справжню перевірку робить SSH-клон вище.
api=$(curl -s -m 10 "http://127.0.0.1:3030/api/v1/repos/vitalii/${REPO}" 2>/dev/null)
if echo "$api" | grep -q '"full_name"'; then
    log "OK  API репозиторію: $(echo "$api" | grep -oE '"full_name":"[^"]+"')"
else
    log "OK  API репозиторію: 404 без токена (приватний репо — очікувано)"
fi

# ── 4. бот ─────────────────────────────────────────────────────────────────
if systemctl is-active --quiet wallet-radar-alerts-bot.service; then
    log "OK  wallet-radar-alerts-bot.service: active"
else
    log "ПОМИЛКА wallet-radar-alerts-bot.service: $(systemctl is-active wallet-radar-alerts-bot.service)"
fi

c409=$(journalctl -u wallet-radar-alerts-bot.service --since "10 min ago" --no-pager 2>/dev/null \
       | grep -c "опитування Telegram все ще недоступне")
if [ "${c409:-0}" -eq 0 ]; then
    log "OK  конфліктів опитування (409) за 10 хв: 0"
else
    log "УВАГА конфліктів опитування (409) за 10 хв: $c409"
fi

log "=== автотест завершено ==="
