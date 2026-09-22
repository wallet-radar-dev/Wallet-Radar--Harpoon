#!/usr/bin/env bash
# PKI для mTLS між кором і сателітами.
#
# Три ролі:
#   ca            — наш власний центр сертифікації; лише він підписує решту
#   satellite-a/b — СЕРВЕРНІ сертифікати (SAN мусить збігатися з іменем хоста в мережі docker)
#   core          — КЛІЄНТСЬКИЙ сертифікат; сателіт не пустить нікого без нього
#
# Приватні ключі НЕ комітяться (див. .gitignore). Скрипт ідемпотентний: наявні файли не чіпає.
set -euo pipefail
cd "$(dirname "$0")"

DAYS=825
mkdir -p out
cd out

if [[ -f ca.crt ]]; then
  echo "CA вже існує — лишаю як є (щоб не зламати вже видані сертифікати)"
else
  echo "→ CA"
  openssl genrsa -out ca.key 4096 2>/dev/null
  openssl req -x509 -new -key ca.key -sha256 -days 3650 -out ca.crt \
    -subj "/O=wallet-radar/CN=wallet-radar-ca" 2>/dev/null
fi

# $1 = ім'я, $2 = SAN (через кому), $3 = extendedKeyUsage
issue() {
  local name="$1" san="$2" eku="$3"
  if [[ -f "$name.crt" ]]; then
    echo "  $name — вже є"
    return
  fi
  echo "→ $name ($eku)"
  openssl genrsa -out "$name.key" 2048 2>/dev/null
  openssl req -new -key "$name.key" -out "$name.csr" -subj "/O=wallet-radar/CN=$name" 2>/dev/null
  # SAN обов'язковий: сучасний TLS ігнорує CN. Без нього кор відкине сертифікат сателіта
  # з "NotValidForName", і це виглядатиме як загадковий збій мережі.
  cat > "$name.ext" <<EOF
subjectAltName = $san
extendedKeyUsage = $eku
basicConstraints = CA:FALSE
EOF
  openssl x509 -req -in "$name.csr" -CA ca.crt -CAkey ca.key -CAcreateserial \
    -out "$name.crt" -days "$DAYS" -sha256 -extfile "$name.ext" 2>/dev/null
  rm -f "$name.csr" "$name.ext"
}

# Імена в SAN = імена сервісів у docker-мережі (саме за ними кор їх шукає).
issue satellite-a "DNS:satellite-a,DNS:localhost,IP:127.0.0.1" serverAuth
issue satellite-b "DNS:satellite-b,DNS:localhost,IP:127.0.0.1" serverAuth
# Кор — клієнт. serverAuth йому не потрібен, і давати його не варто: сертифікат має вміти
# рівно те, для чого виданий.
issue core "DNS:core" clientAuth

chmod 600 ./*.key
echo
echo "Готово. Видано:"
ls -1 ./*.crt | sed 's/^/  /'
echo
echo "Перевірка ланцюжка довіри:"
for c in satellite-a satellite-b core; do
  printf "  %-12s " "$c"
  openssl verify -CAfile ca.crt "$c.crt" 2>&1 | sed 's/.*: //'
done
