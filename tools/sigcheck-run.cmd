@echo off
REM ─────────────────────────────────────────────────────────────────────────────
REM sigcheck — дозрівання журналу сигналів раз на добу + звіт у Telegram.
REM
REM Навіщо. selltape пише в журнал КОЖЕН сигнал із ціною на момент події. Сам по собі
REM цей файл нічого не доводить: потрібен той, хто через 1/7/30/90 днів повернеться до
REM старих записів і порахує, що сталося з ціною відносно ринку. Це він.
REM
REM Мережа тут коштує майже нічого (DefiLlama, без ключів), але ходить бін лише за
REM тими точками, яких ще немає: повторний прогін того самого дня не витрачає жодного
REM запиту. Тому добовий крок — із запасом.
REM
REM ПАУЗА ЧЕРЕЗ Start-Sleep, А НЕ `timeout`: `timeout /t` падає при перенаправленому
REM вводі (запуск іде з .vbs із виводом у файл), і цикл крутиться всуху. Так уже
REM згоріло ~90 тис. зайвих запитів Etherscan на groupsig — повторювати не будемо.
REM
REM Журнал лежить ПОРУЧ ІЗ ДАТАСЕТОМ, з якого рахувались сигнали (той самий шлях, що
REM WR_DATASET у selltape-run.cmd). Якщо ці два шляхи розійдуться, sigcheck побачить
REM порожньо й завершиться кодом 1 — мовчазного «нуль сигналів, усе гаразд» не буде.
REM ─────────────────────────────────────────────────────────────────────────────
set "ROOT=C:\Users\StarOsta\wallet-tracker-CODE-backup-2026-09-08"
set "LOG=D:\wr-live\sigcheck.log"
set "INTERVAL_SEC=86400"

set "SIGNAL_JOURNAL_DIR=D:\wallet-radar-backup\current\dataset"

REM Горизонти: доба ловить миттєву реакцію, 7 і 30 — те, заради чого все робиться,
REM 90 показує, чи ефект не розсмоктується. MIN_N — поріг, нижче якого присуду немає.
set "HORIZONS=1,7,30,90"
set "MIN_N=30"
REM Базовий актив, проти якого рахується надлишок. ETH, а не BTC: наш всесвіт — ERC-20.
set "BENCH=coingecko:ethereum"

REM Сховище довідників: без цих змінних біни падають із поясненням, а не працюють
REM впівсили. Слеші ПРЯМІ: зворотні з'їдаються при читанні оболонкою.
set "WR_DB=D:/wr-live/store/wallet-radar.db"
set "WR_SEED=D:/wr-live/store/seed"
set "WR_ARCHIVE_ROOTS=D:/wallet-radar-backup/current"
set "WR_REPO=C:/Users/StarOsta/wallet-tracker-CODE-backup-2026-09-08"

cd /d "%ROOT%"

:loop
if not exist "D:\wr-live" (
  echo [%date% %time%] диска D: немає - чекаю >> "%ROOT%\sigcheck-fallback.log"
  powershell -NoProfile -Command "Start-Sleep -Seconds 300"
  goto loop
)

tasklist /fi "imagename eq sigcheck.exe" 2>nul | find /i "sigcheck.exe" >nul
if not errorlevel 1 (
  echo [%date% %time%] попереднiй прогiн ще йде - пропускаю >> "%LOG%"
  powershell -NoProfile -Command "Start-Sleep -Seconds 600"
  goto loop
)

echo [%date% %time%] === прогiн === >> "%LOG%"

REM Спершу нульова база: без неї присуд неможливий у принципі — порівнювати сигнал
REM буде нема з чим. Рахується з локальних цінових рядів, без мережі, за секунду.
"%ROOT%\backfill\target\release\nullbase.exe" "%ROOT%" >> "%LOG%" 2>&1
if errorlevel 1 echo [%date% %time%] nullbase: ПОМИЛКА, код %errorlevel% >> "%LOG%"

"%ROOT%\backfill\target\release\sigcheck.exe" "%ROOT%" >> "%LOG%" 2>&1
if errorlevel 1 echo [%date% %time%] ПОМИЛКА, код %errorlevel% >> "%LOG%"

REM -- ПАУЗА ЗА СТІННИМ ГОДИННИКОМ (виправлено 26.09.2026) --
REM `Start-Sleep -Seconds N` не рахує час, коли система СПАЛА. Цей ноут засинає по
REM кілька разів на добу, тому пауза «на 6 годин» тривала 33 години реального часу:
REM цикл виглядав налаштованим, а дані старіли. Тепер чекаємо до ЦІЛЬОВОГО ЧАСУ
REM короткими кроками — після пробудження годинник уже за межею, і коло йде далі.
powershell -NoProfile -Command "$t=(Get-Date).AddSeconds(%INTERVAL_SEC%); while((Get-Date) -lt $t){ Start-Sleep -Seconds 30 }"
goto loop
