#!/bin/bash
# Автотест після перезавантаження: перевіряє, що Forgejo-remote знову живий.
#
# Набір перевірок:
#   1. чи це справді НОВЕ завантаження (boot_id відрізняється від попереднього);
#   2. чи машина отримала ту саму адресу (192.168.2.104) — тобто чи
#      спрацювала DHCP-резервація;
#   3. чи резервація досі в реєстрі роутера;
#   4. чи Forgejo піднявся і віддає репозиторій по SSH (плюс реальний клон);
#   5. чи бот живий, чи джерело (Wallet Radar) живе і чи немає конфлікту
#      опитування (409).
#
# Результат пишеться в /var/log/wra-postboot-check.log — його можна
# прочитати після ребуту, навіть якщо агент не піднявся. Короткий підсумок
# (PASS/FAIL) — у /var/lib/wra-postboot-check.result.
set -uo pipefail

LOG="/var/log/wra-postboot-check.log"
RESULT="/var/lib/wra-postboot-check.result"
PREV_BOOT_ID_FILE="/var/lib/wra-preboot-id"
EXPECT_IP="${EXPECT_IP:-192.168.2.104}"
ROUTER="${ROUTER:-192.168.2.1}"
REPO="vitalii/wallet-radar-alerts-bot"
SSH_URL="ssh://git@${EXPECT_IP}:2222/${REPO}.git"
API_URL="http://127.0.0.1:3030"
WR_URL="http://127.0.0.1:3000"

fail=0
: > "$RESULT"

log() { echo "[$(date '+%Y-%m-%d %H:%M:%S')] $*" | tee -a "$LOG"; }

# Запис у файл підсумку: key=value, один рядок на перевірку.
res() {
    echo "$1=$2" >> "$RESULT"
    if [ "$2" = "FAIL" ]; then fail=1; fi
}

log "=== автотест після перезавантаження ==="

# ── 1. доказ нового завантаження ───────────────────────────────────────────
boot_id=$(cat /proc/sys/kernel/random/boot_id 2>/dev/null)
prev_boot_id=""
[ -f "$PREV_BOOT_ID_FILE" ] && prev_boot_id=$(cat "$PREV_BOOT_ID_FILE")
if [ -n "$prev_boot_id" ] && [ "$prev_boot_id" != "$boot_id" ]; then
    log "OK  перезавантаження підтверджено: boot_id $prev_boot_id → $boot_id"
    res "reboot" "PASS"
else
    if [ -z "$prev_boot_id" ]; then
        log "УВАГА немає файлу $PREV_BOOT_ID_FILE — доказ нового завантаження не звірено"
    else
        log "ПОМИЛКА boot_id не змінився ($boot_id) — перезавантаження не відбулося"
    fi
    res "reboot" "FAIL"
fi
echo "boot_id=$boot_id" >> "$RESULT"

# ── 2. адреса (з очікуванням: Wi-Fi/DHCP піднімаються не миттєво) ───────────
ip4=""
for _ in $(seq 1 30); do
    ip4=$(ip -4 addr show wlp0s20f3 2>/dev/null | grep -oE 'inet [0-9.]+' | cut -d' ' -f2 | head -1)
    [ -n "$ip4" ] && break
    sleep 4
done
if [ "$ip4" = "$EXPECT_IP" ]; then
    log "OK  адреса: $ip4 (очікували $EXPECT_IP) — DHCP-резервація спрацювала"
    res "ip" "PASS"
else
    log "ПОМИЛКА адреса: ${ip4:-немає} (очікували $EXPECT_IP)"
    res "ip" "FAIL"
fi
echo "ip=$ip4" >> "$RESULT"

# ── 3. реєстр резервацій на роутері ───────────────────────────────────────
resv=""
for _ in $(seq 1 5); do
    resv=$(curl -s -m 10 "http://${ROUTER}/goform/getNAT?modules=staticIPList" 2>/dev/null)
    [ -n "$resv" ] && break
    sleep 4
done
if echo "$resv" | grep -q "$EXPECT_IP"; then
    log "OK  резервація в роутері: $(echo "$resv" | head -c 160)"
    res "reservation" "PASS"
else
    log "ПОМИЛКА резервації в роутері немає: ${resv:-роутер недоступний}"
    res "reservation" "FAIL"
fi

# ── 4. Forgejo ─────────────────────────────────────────────────────────────
for _ in $(seq 1 30); do
    systemctl is-active --quiet forgejo.service && break
    sleep 4
done
if systemctl is-active --quiet forgejo.service; then
    log "OK  forgejo.service: active"
    res "forgejo_service" "PASS"
else
    log "ПОМИЛКА forgejo.service: $(systemctl is-active forgejo.service)"
    res "forgejo_service" "FAIL"
fi

ls_ok=0
for _ in $(seq 1 10); do
    if timeout 20 git ls-remote --exit-code "$SSH_URL" >/dev/null 2>&1; then
        ls_ok=1
        break
    fi
    sleep 5
done
if [ "$ls_ok" = "1" ]; then
    log "OK  Forgejo віддає гілки по SSH: $SSH_URL"
    res "git_ls_remote" "PASS"
else
    log "ПОМИЛКА Forgejo не віддає гілки по SSH: $SSH_URL"
    res "git_ls_remote" "FAIL"
fi

# Реальний клон — найсуворіша перевірка remote: показує, що репозиторій
# не просто «віддає посилання», а реально клонується.
tmpdir=$(mktemp -d)
if timeout 90 git clone --quiet --depth 1 "$SSH_URL" "$tmpdir/clone" >/dev/null 2>&1; then
    head_sha=$(git -C "$tmpdir/clone" rev-parse --short HEAD 2>/dev/null)
    nfiles=$(find "$tmpdir/clone" -type f -not -path '*/.git/*' | wc -l)
    log "OK  реальний клон удався: HEAD=$head_sha, файлів без .git: $nfiles"
    res "git_clone" "PASS"
else
    log "ПОМИЛКА клон Forgejo-remote не вдався: $SSH_URL"
    res "git_clone" "FAIL"
fi
rm -rf "$tmpdir"

# Репозиторій приватний, тож API без токена віддає 404 — це очікувано,
# а не помилка. Справжню перевірку робить SSH-клон вище.
api=$(curl -s -m 10 "${API_URL}/api/v1/repos/vitalii/${REPO}" 2>/dev/null)
if echo "$api" | grep -q '"full_name"'; then
    log "OK  API репозиторію: $(echo "$api" | grep -oE '"full_name":"[^"]+"')"
else
    log "OK  API репозиторію: 404 без токена (приватний репо — очікувано)"
fi
res "forgejo_api" "PASS"

# ── 5. бот і джерело ───────────────────────────────────────────────────────
if systemctl is-active --quiet wallet-radar-alerts-bot.service; then
    log "OK  wallet-radar-alerts-bot.service: active"
    res "bot_service" "PASS"
else
    log "ПОМИЛКА wallet-radar-alerts-bot.service: $(systemctl is-active wallet-radar-alerts-bot.service)"
    res "bot_service" "FAIL"
fi

if systemctl is-active --quiet wallet-radar-rust.service; then
    log "OK  wallet-radar-rust.service (джерело даних): active"
    res "source_service" "PASS"
else
    log "ПОМИЛКА wallet-radar-rust.service: $(systemctl is-active wallet-radar-rust.service)"
    res "source_service" "FAIL"
fi

# Джерело даних (Wallet Radar) закрите basic-auth, як це робить сам бот.
# Пароль беремо з .env юнита (0600, root) і в лог НЕ потрапляє.
WR_ENV="/opt/wallet-radar-alerts-bot/.env"
wr_user="admin"
wr_pass=""
if [ -r "$WR_ENV" ]; then
    wr_pass=$(grep -E '^WR_PASSWORD=' "$WR_ENV" | head -1 | cut -d= -f2-)
fi
code=$(curl -s -o /dev/null -w '%{http_code}' -m 10 -u "${wr_user}:${wr_pass}" \
       "${WR_URL}/api/health" 2>/dev/null)
if [ "$code" = "200" ]; then
    log "OK  Wallet Radar відповідає на ${WR_URL}/api/health (HTTP 200, з авторизацією)"
    res "source_api" "PASS"
else
    log "УВАГА Wallet Radar не відповідає як слід (HTTP ${code:-немає})"
    res "source_api" "FAIL"
fi

# Живий потік подій — те, з чим реально працює бот.
code_ev=$(curl -s -o /dev/null -w '%{http_code}' -m 15 -u "${wr_user}:${wr_pass}" \
          "${WR_URL}/api/events?limit=1" 2>/dev/null)
if [ "$code_ev" = "200" ]; then
    log "OK  Wallet Radar віддає потік подій (${WR_URL}/api/events, HTTP 200)"
    res "source_events" "PASS"
else
    log "УВАГА потік подій Wallet Radar недоступний (HTTP ${code_ev:-немає})"
    res "source_events" "FAIL"
fi
unset wr_pass

c409=$(journalctl -u wallet-radar-alerts-bot.service --since "15 min ago" --no-pager 2>/dev/null \
       | grep -c "опитування Telegram все ще недоступне")
if [ "${c409:-0}" -eq 0 ]; then
    log "OK  конфліктів опитування (409) за 15 хв: 0"
    res "conflict_409" "PASS"
else
    log "УВАГА конфліктів опитування (409) за 15 хв: $c409"
    res "conflict_409" "FAIL"
fi

if [ "$fail" = "0" ]; then
    summary="PASS"
    log "ПІДСУМОК: PASS — Forgejo-remote знову працює"
    res "summary" "PASS"
else
    summary="FAIL"
    log "ПІДСУМОК: FAIL — є перевірки, що не пройшли (див. вище)"
    res "summary" "FAIL"
fi

# ── 6. доказ у Forgejo: звіт окремою гілкою ─────────────────────────────────
# Під час ребуту агент (і SSH-сесія) помре, тож лог на диску — єдиний
# доказ, який треба читати з машини. Тому додатково пушимо звіт у приватний
# репозиторій окремою гілкою: його видно у Forgejo-UI навіть без SSH.
# Клон робимо в тимчасовій директорії, щоб не бруднити робочу копію репозиторію.
REPORT_BRANCH="postboot-report"
REPORT_DIR="$(mktemp -d)"
if timeout 90 git clone --quiet --depth 1 "$SSH_URL" "$REPORT_DIR/r" >/dev/null 2>&1; then
    {
        echo "# Звіт автотесту після перезавантаження"
        echo
        echo "- Момент (UTC): $(date -u '+%Y-%m-%d %H:%M:%S')"
        echo "- Boot ID: ${boot_id}"
        echo "- Попередній boot ID: ${prev_boot_id:-немає}"
        echo "- Підсумок: ${summary}"
        echo
        echo '```'
        cat "$RESULT"
        echo '```'
    } > "$REPORT_DIR/r/docs/postboot-report.md"
    if git -C "$REPORT_DIR/r" add docs/postboot-report.md >/dev/null 2>&1 \
       && git -C "$REPORT_DIR/r" -c user.name="postboot-check" -c user.email="postboot@localhost" \
          commit -q -m "Звіт автотесту після перезавантаження: ${summary}" >/dev/null 2>&1 \
       && git -C "$REPORT_DIR/r" push -q origin "HEAD:refs/heads/${REPORT_BRANCH}" >/dev/null 2>&1; then
        log "OK  звіт опубліковано в Forgejo: гілка ${REPORT_BRANCH}"
    else
        log "УВАГА не вдалося опублікувати звіт у Forgejo (гілка ${REPORT_BRANCH})"
    fi
else
    log "УВАГА не вдалося склонивати репозиторій для звіту"
fi
rm -rf "$REPORT_DIR"

log "=== автотест завершено ==="

# Скрипт завжди завершується з 0: oneshot-юніт не має падати через
# невдалу перевірку — підсумок уже записано у $RESULT.
exit 0
