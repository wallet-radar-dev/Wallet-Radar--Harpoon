#!/usr/bin/env bash
# Живий прогін бота проти справжнього Wallet Radar на :3000.
#
# Що робить:
#   1. піднімає мок Telegram API (справжнього токена на цій ВМ немає);
#   2. запускає бота, підключеного до живого Wallet Radar;
#   3. чекає, поки бот збере зріз і стрічку;
#   4. витягує з відповідей Wallet Radar УСІ адреси й tx-hash;
#   5. перевіряє, що жодна з них не зустрічається в тому, що бот показав.
#
# Крок 5 — головна перевірка на живих даних: синтетичні фікстури в golden-тесті
# доводять логіку, а цей крок доводить, що в реальному потоці теж немає витоку.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

WR_BASE="${WR_BASE:-http://127.0.0.1:3000}"
WR_USER="${WR_USER:-admin}"
WR_PASS="${WR_PASSWORD:-}"
MOCK_ADDR="${MOCK_ADDR:-127.0.0.1:8099}"
RUN_SECS="${RUN_SECS:-25}"
OUT="/tmp/wra-messages.log"
CFG="/tmp/wra-live.toml"

if [[ -z "$WR_PASS" ]]; then
  set -a; . /opt/wallet-tracker/.env 2>/dev/null || true; set +a
  WR_PASS="${ADMIN_PASSWORD:-${APP_PASSWORD:-}}"
fi
if [[ -z "$WR_PASS" ]]; then
  echo "ПОМИЛКА: не знайдено пароль Wallet Radar (WR_PASSWORD або /opt/wallet-tracker/.env)" >&2
  exit 1
fi

echo "→ збираю бота"
cargo build --quiet --bin bot --bin mock-telegram

cat > "$CFG" <<EOF
[bot]
name = "wallet-radar-alerts-live"
token_env = "TELEGRAM_BOT_TOKEN"
default_language = "uk"
log_level = "info"

[telegram]
api_url = "http://$MOCK_ADDR"

[wr]
base_url = "$WR_BASE"
username = "$WR_USER"
password_env = "WR_PASSWORD"
poll_interval_secs = 10
event_poll_interval_secs = 5
ws_path = "/ws"

[storage]
backend = "sqlite"
sqlite_path = "/tmp/wra-live.db"

[privacy]
mask = "приховано"

[admin]
usernames = ["vitalii_vedana"]

[[alerts.rules]]
kind = "whale_accumulation"
min_participants = 2
min_total_value_usd = 1000
windows = ["1h"]
assets = []
severity = "medium"
EOF

rm -f "$OUT" /tmp/wra-live.db
echo "→ піднімаю мок Telegram на $MOCK_ADDR"
./target/debug/mock-telegram "$MOCK_ADDR" "$OUT" >/tmp/wra-mock.log 2>&1 &
MOCK_PID=$!
trap 'kill $MOCK_PID 2>/dev/null || true' EXIT
sleep 1

echo "→ запускаю бота проти $WR_BASE"
start_bot() {
  TELEGRAM_BOT_TOKEN="dummy-live-token" WR_PASSWORD="$WR_PASS" \
    ./target/debug/bot "$CFG" >>/tmp/wra-bot.log 2>&1 &
  BOT_PID=$!
}

: > /tmp/wra-bot.log

# Фаза 1: бот створює схему БД.
echo "→ фаза 1: піднімаю бота, щоб створити схему"
start_bot
sleep 4
kill -TERM $BOT_PID 2>/dev/null || true
sleep 1
kill $BOT_PID 2>/dev/null || true

# Фаза 2: підсідаємо тестового користувача і чистимо alert_log, щоб алерти
# сформувалися знову (вже з одержувачем).
if [[ "${SEED_USER:-1}" == "1" ]]; then
  python3 - <<'PY'
import sqlite3, time
db = sqlite3.connect("/tmp/wra-live.db")
now = int(time.time() * 1000)
db.execute("INSERT OR REPLACE INTO users (chat_id, user_id, username, role, language, timezone, timeframe, muted, blocked, state, chat_ref, created_at, updated_at) VALUES (424242, 424242, 'smoke', 'viewer', 'uk', 'UTC', '24h', 0, 0, 'active', 'chat_smoke', ?, ?)", (now, now))
for topic in ("whale", "group", "liquidation"):
    db.execute("INSERT OR REPLACE INTO subscriptions (chat_id, topic, min_severity) VALUES (424242, ?, 'medium')", (topic,))
db.execute("DELETE FROM alert_log")
db.execute("DELETE FROM deliveries")
db.commit()
print("→ тестовий користувач 424242 підписаний на whale/group/liquidation")
PY
fi

# Фаза 3: основний прогін — алерти формуються й ДОСТАВЛЯЮТЬСЯ.
echo "→ фаза 3: запускаю бота з одержувачем"
start_bot
sleep "$RUN_SECS"
kill -TERM $BOT_PID 2>/dev/null || true
sleep 1
kill $BOT_PID 2>/dev/null || true

echo
echo "=== лог бота (останні 25 рядків) ==="
tail -25 /tmp/wra-bot.log || true

echo
echo "=== повідомлення, які бот «надіслав» ==="
if [[ -s "$OUT" ]]; then
  cat "$OUT"
else
  echo "(повідомлень немає — у цьому прогоні ніхто не підписаний і не було алертів)"
fi

echo
echo "=== корпус заборонених токенів із живих відповідей Wallet Radar ==="
CORPUS=/tmp/wra-corpus.txt
: > "$CORPUS"
for ep in /api/events /api/capital-groups /api/signals /api/whale-assets /api/cross-asset /api/asset-forensics; do
  curl -s -m 10 -u "$WR_USER:$WR_PASS" "$WR_BASE$ep" >> "$CORPUS" || true
done
# Унікальні hex-рядки довжиною 40 або 64 — це адреси й tx-hash.
grep -oE '0x[0-9a-fA-F]{40}|0x[0-9a-fA-F]{64}' "$CORPUS" | tr 'A-F' 'a-f' | sort -u > /tmp/wra-tokens.txt
COUNT=$(wc -l < /tmp/wra-tokens.txt)
echo "унікальних адрес/tx у відповідях джерела: $COUNT"

echo
echo "=== перевірка: чи просочилося щось у повідомлення бота ==="
LEAK=0
if [[ -s "$OUT" ]]; then
  while read -r token; do
    [[ -z "$token" ]] && continue
    if grep -qF "$token" "$OUT"; then
      echo "ВИТІК: $token є у вихідних повідомленнях"
      LEAK=1
    fi
  done < /tmp/wra-tokens.txt
fi
if [[ "$LEAK" == "0" ]]; then
  echo "OK: жодна адреса/tx з живого джерела не потрапила у вихід"
else
  echo "FAIL: є витік" >&2
  exit 1
fi

echo
echo "=== перевірка: чи тримає бот сирі адреси у своєму стані ==="
if [[ -f /tmp/wra-live.db ]]; then
  # У БД бот має зберігати лише агрегати; перевіряємо таблиці alert_log і snapshots.
  python3 - <<'PY'
import sqlite3
tokens = [t.strip() for t in open("/tmp/wra-tokens.txt") if t.strip()]
db = sqlite3.connect("/tmp/wra-live.db")
total = 0
for tbl, col in (("alert_log", "payload"), ("snapshots", "payload"), ("deliveries", "text")):
    try:
        rows = db.execute(f"SELECT CAST({col} AS TEXT) FROM {tbl}").fetchall()
    except Exception as e:
        print(f"{tbl}: перевірка неможлива: {e}")
        continue
    hits = 0
    for (value,) in rows:
        if not value:
            continue
        low = value.lower()
        hits += sum(1 for t in tokens if t in low)
    total += hits
    print(f"{tbl}: рядків={len(rows)}, збігів із корпусом реальних адрес/tx: {hits}")
print("ВИСНОВОК:", "чисто — у стані бота немає жодної реальної адреси/tx" if total == 0
      else f"ЗНАЙДЕНО {total} збігів — у стані є сирі дані!")
PY
fi
echo
echo "ГОТОВО. Логи: /tmp/wra-bot.log · повідомлення: $OUT · корпус: /tmp/wra-tokens.txt"
