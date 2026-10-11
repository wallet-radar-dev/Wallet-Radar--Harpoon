@echo off
REM ─────────────────────────────────────────────────────────────────────────────
REM Щоденна ДОКАЧКА transfers.csv для членів груп (мережа eth).
REM
REM Навіщо: emissionShare і баланси groupsig тягне з Etherscan наживо, а от ФАЗУ
REM («набирає / роздає / тримає») рахує з transfers.csv за останні 90 днів. Якщо
REM файл не поповнювати, фаза тихо застаряє — числа виглядають свіжими, а означають
REM позаминулий місяць.
REM
REM Порядок: спершу перевести зроблені пари в докачку (done -> part зі збереженням
REM останнього блоку), потім зібрати. Без першого кроку backfill2 пропустить усе.
REM
REM Замір 13.09.2026: 500 гаманців -> 67% порожні, ~2 запити на гаманець у середньому.
REM На 61439 членів це ~120 тис. запитів = 15% денної квоти 8 ключів. Тому РАЗ НА ДОБУ.
REM
REM ПАУЗА ЧЕРЕЗ Start-Sleep, А НЕ `timeout`: `timeout /t` не працює при перенаправленому
REM вводі і падає миттєво, перетворюючи цикл на холостий хід (14.09 це коштувало 102 МБ
REM логу за ніч і сотень зайвих запитів).
REM ─────────────────────────────────────────────────────────────────────────────
set "ROOT=C:\Users\StarOsta\wallet-tracker-CODE-backup-2026-09-08"
set "LOG=D:\wr-live\backfill2.log"
set "INTERVAL_SEC=86400"

REM ── СХОВИЩЕ ДОВІДНИКІВ ────────────────────────────────────────────────────
REM Після переїзду адрес/порогів/стратегій у SQLite бінарники БЕЗ цих змінних не
REM стартують — падають із поясненням, а не працюють впівсили. Дублюємо тут явно
REM (як і DATASET_DIR): якщо deploy\store.env колись перезапишуть, запуск однаково
REM лишиться правильним.
REM Слеші ПРЯМІ: зворотні з'їдаються при читанні оболонкою.
set "WR_DB=D:/wr-live/store/wallet-radar.db"
set "WR_SEED=D:/wr-live/store/seed"
set "WR_ARCHIVE_ROOTS=D:/wallet-radar-backup/current"
set "WR_REPO=C:/Users/StarOsta/wallet-tracker-CODE-backup-2026-09-08"


cd /d "%ROOT%"

:loop
if not exist "D:\wr-live" (
  echo [%date% %time%] диска D: немає - чекаю >> "%ROOT%\backfill-fallback.log"
  powershell -NoProfile -Command "Start-Sleep -Seconds 300"
  goto loop
)

REM Два збирачі писали б у той самий transfers.csv і той самий прогрес.
tasklist /fi "imagename eq backfill2.exe" 2>nul | find /i "backfill2.exe" >nul
if not errorlevel 1 (
  echo [%date% %time%] попередній збір ще йде - пропускаю >> "%LOG%"
  powershell -NoProfile -Command "Start-Sleep -Seconds 1800"
  goto loop
)

REM Ключі беремо з deploy\backfill.env: backfill2 читає лише змінну оточення.
for /f "tokens=1,* delims==" %%A in ('findstr /b "BACKFILL_ETHERSCAN_KEYS=" "%ROOT%\deploy\backfill.env"') do set "ETHERSCAN_API_KEYS=%%B"

echo [%date% %time%] === підготовка докачки === >> "%LOG%"
python "%ROOT%\tools\backfill-topup.py" "%ROOT%" >> "%LOG%" 2>&1

echo [%date% %time%] === збір (eth, conc=12) === >> "%LOG%"
set "APP_ROOT=%ROOT%"
set "CHAINS=eth"
set "WALLETS_FILE=%ROOT%\dataset\backfill\grp-wallets-eth.txt"
set "CONCURRENCY=12"
"%ROOT%\backfill\target\release\backfill2.exe" >> "%LOG%" 2>&1

echo [%date% %time%] коло завершено, наступне через %INTERVAL_SEC% с >> "%LOG%"

REM ── ПАУЗА ЗА СТІННИМ ГОДИННИКОМ (виправлено 26.09.2026) ──
REM `Start-Sleep -Seconds N` не рахує час, коли система СПАЛА. Цей ноут засинає по
REM кілька разів на добу (події Kernel-Power 42/107), тому добова пауза тривала майже
REM дві доби реального часу: цикл виглядав налаштованим, а дані старіли. Тепер чекаємо
REM до ЦІЛЬОВОГО ЧАСУ короткими кроками — після пробудження годинник уже за межею,
REM і коло йде далі без затримки.
powershell -NoProfile -Command "$t=(Get-Date).AddSeconds(%INTERVAL_SEC%); while((Get-Date) -lt $t){ Start-Sleep -Seconds 30 }"
goto loop
