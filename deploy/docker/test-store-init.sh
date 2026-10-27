#!/bin/sh
# Регресійний тест охоронців store-init.sh: наявну базу не можна змінити, перезаписати чи видалити.
# Запуск (з кореня репо, після `docker compose build core`):
#   docker run --rm --entrypoint sh -e WR_DB=/app/store/wallet-radar.db -e WR_SEED=/app/seed \
#     -v "$PWD/deploy/docker/test-store-init.sh:/t.sh:ro" wallet-radar-core -c "sed 's/\r\$//' /t.sh | sh"
# Прогін сценаріїв store-init.sh. Кожен починає з чистих /app/store і /app/seed.
DB=/app/store/wallet-radar.db
pass=0; fail=0
reset() { rm -rf /app/store /app/seed; mkdir -p /app/store /app/seed; }
check() { if [ "$2" = "$3" ]; then echo "  ok   $1"; pass=$((pass+1)); else echo "  FAIL $1 (got '$2', want '$3')"; fail=$((fail+1)); fi; }
run() { store-init.sh >/tmp/out 2>&1; echo $?; }
sum() { [ -e "$1" ] && md5sum "$1" | cut -d' ' -f1 || echo none; }

echo "A. наявна 'реальна' база не змінюється"
reset; head -c 5000 /dev/urandom > $DB; before=$(sum $DB)
check "exit 0" "$(run)" 0; check "вміст той самий" "$(sum $DB)" "$before"
echo "A2. те саме, коли поруч є seed-файли і готова база"
reset; head -c 5000 /dev/urandom > $DB; before=$(sum $DB); echo '[]' > /app/seed/constants.json; head -c 900 /dev/urandom > /app/seed/wallet-radar.db
check "exit 0" "$(run)" 0; check "вміст той самий" "$(sum $DB)" "$before"

echo "B. чисто → порожня база; повторний запуск її не чіпає"
reset
check "exit 0" "$(run)" 0; [ -s $DB ] && s=yes || s=no; check "база створена" "$s" yes
before=$(sum $DB); check "повтор exit 0" "$(run)" 0; check "повтор не змінив" "$(sum $DB)" "$before"
check "тимчасових файлів не лишилось" "$(ls /app/store | grep -c tmp)" 0

echo "C. осиротілий -wal без основного файла → відмова, нічого не створено"
reset; echo x > $DB-wal
check "exit 1" "$(run)" 1; check "бази не створено" "$(sum $DB)" none; check "wal на місці" "$(sum $DB-wal)" "$(echo x | md5sum | cut -d' ' -f1)"

echo "D. порожній файл за шляхом бази → відмова, файл не чіпаємо"
reset; : > $DB
check "exit 1" "$(run)" 1; [ -e $DB ] && [ ! -s $DB ] && s=intact || s=changed; check "0 байт лишився" "$s" intact

echo "D2. битий симлінк за шляхом бази → відмова"
reset; ln -s /nonexistent $DB
check "exit 1" "$(run)" 1; [ -L $DB ] && s=intact || s=changed; check "симлінк лишився" "$s" intact

echo "E. залишок обірваного запуску не стає базою"
reset; echo junk > $DB.tmp.999
check "exit 0" "$(run)" 0; check "залишок прибрано" "$(ls /app/store | grep -c 'tmp')" 0; [ -s $DB ] && s=yes || s=no; check "база створена" "$s" yes

echo "F. готова база з seed копіюється байт-у-байт"
reset; head -c 4000 /dev/urandom > /app/seed/wallet-radar.db
check "exit 0" "$(run)" 0; check "копія ідентична" "$(sum $DB)" "$(sum /app/seed/wallet-radar.db)"; check "оригінал у seed цілий" "$(wc -c < /app/seed/wallet-radar.db)" 4000

echo "G. гонка: хтось створив базу, поки йшла побудова → її не затираємо"
reset; mkdir -p /fakebin
cat > /fakebin/wr-import <<'W'
#!/bin/sh
echo REAL-DATA-FROM-OTHER-PROCESS > /app/store/wallet-radar.db
exec /usr/local/bin/wr-import "$@"
W
chmod +x /fakebin/wr-import
PATH=/fakebin:$PATH store-init.sh >/tmp/out 2>&1; rc=$?
check "exit 1" "$rc" 1; check "чужа база цілa" "$(cat $DB)" REAL-DATA-FROM-OTHER-PROCESS; check "наш тимчасовий прибрано" "$(ls /app/store | grep -c tmp)" 0

echo; echo "passed=$pass failed=$fail"; [ $fail -eq 0 ]
