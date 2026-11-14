#!/usr/bin/env bash
# Піднімає Postgres для Wallet Radar Alerts Bot (Docker, локально).
#
#   sudo bash tools/postgres_setup.sh
#
# Що робить:
#   * створює пароль у /opt/wallet-radar-alerts-bot/secrets/pg_password (0600, root);
#   * запускає контейнер `wr-postgres` на 127.0.0.1:5433 із паролем із файлу
#     (POSTGRES_PASSWORD_FILE — пароль не світиться в `docker inspect`);
#   * створює базу `wallet_radar` і тестову `wallet_radar_test`;
#   * додає POSTGRES_URL у .env бота (якщо ще немає).
#
# Сховище бота вмикається в config.toml:
#     [storage]
#     backend = "postgres"

set -euo pipefail

APP_DIR="${APP_DIR:-/opt/wallet-radar-alerts-bot}"
SECRETS_DIR="$APP_DIR/secrets"
PW_FILE="$SECRETS_DIR/pg_password"
CONTAINER="${CONTAINER:-wr-postgres}"
PORT="${PORT:-5433}"
DB="${DB:-wallet_radar}"
TEST_DB="${TEST_DB:-wallet_radar_test}"
USER_NAME="${PGUSER:-wr}"
VOLUME="${VOLUME:-wr-postgres-data}"

if [[ $EUID -ne 0 ]]; then
  echo "запускати через sudo" >&2
  exit 1
fi

command -v docker >/dev/null || { echo "docker не встановлено" >&2; exit 1; }

# 1. Пароль (генерується лише якщо файла ще немає).
mkdir -p "$SECRETS_DIR"
chmod 700 "$SECRETS_DIR"
if [[ ! -f "$PW_FILE" ]]; then
  head -c 24 /dev/urandom | base64 | tr -d '/+=' | head -c 24 > "$PW_FILE"
  chmod 600 "$PW_FILE"
  chown root:root "$PW_FILE"
  echo "створено новий пароль у $PW_FILE"
else
  echo "пароль уже є: $PW_FILE"
fi

# 2. Контейнер.
if docker ps -a --format '{{.Names}}' | grep -qx "$CONTAINER"; then
  echo "контейнер $CONTAINER уже існує — перезапускаю"
  docker rm -f "$CONTAINER" >/dev/null
fi
docker run -d --name "$CONTAINER" \
  --restart unless-stopped \
  -p "127.0.0.1:${PORT}:5432" \
  -e POSTGRES_USER="$USER_NAME" \
  -e POSTGRES_DB="$DB" \
  -e POSTGRES_PASSWORD_FILE=/run/secrets/pg_password \
  -v "$PW_FILE:/run/secrets/pg_password:ro" \
  -v "$VOLUME:/var/lib/postgresql/data" \
  postgres:16-alpine >/dev/null
echo "контейнер $CONTAINER піднято на 127.0.0.1:${PORT}"

# 3. Чекаємо готовності й створюємо тестову базу.
for _ in $(seq 1 30); do
  if docker exec -e PGPASSWORD="$(cat "$PW_FILE")" "$CONTAINER" \
       psql -U "$USER_NAME" -d "$DB" -c 'SELECT 1' >/dev/null 2>&1; then
    break
  fi
  sleep 1
done
docker exec -e PGPASSWORD="$(cat "$PW_FILE")" "$CONTAINER" \
  psql -U "$USER_NAME" -d postgres \
  -c "SELECT 1 FROM pg_database WHERE datname='$TEST_DB'" -t | grep -q 1 \
  || docker exec -e PGPASSWORD="$(cat "$PW_FILE")" "$CONTAINER" \
       psql -U "$USER_NAME" -d postgres -c "CREATE DATABASE $TEST_DB" >/dev/null
echo "бази: $DB, $TEST_DB"

# 4. URL для бота — у .env, не в конфіг.
URL="postgres://$USER_NAME:$(cat "$PW_FILE")@127.0.0.1:${PORT}/$DB"
if [[ -f "$APP_DIR/.env" ]]; then
  if grep -q '^POSTGRES_URL=' "$APP_DIR/.env"; then
    sed -i "s|^POSTGRES_URL=.*|POSTGRES_URL=$URL|" "$APP_DIR/.env"
    echo "POSTGRES_URL оновлено у $APP_DIR/.env"
  else
    printf 'POSTGRES_URL=%s\n' "$URL" >> "$APP_DIR/.env"
    echo "POSTGRES_URL додано у $APP_DIR/.env"
  fi
  chmod 600 "$APP_DIR/.env"
fi

echo
echo "готово. Перемикання бота:"
echo "  sed -i 's/^backend = .*/backend = \"postgres\"/' $APP_DIR/config.toml"
echo "  sudo systemctl restart wallet-radar-alerts-bot.service"
