@echo off
REM ─────────────────────────────────────────────────────────────────────────────
REM groupwatch — алерт РІВНЯ ГРУПИ: коли група капіталу міняє намір щодо активу.
REM
REM Навіщо окремо від решти моніторів. Усі попередні стежать за адресою або активом;
REM цей — за ГРУПОЮ: десятки гаманців, зведені в одну групу, разом почали роздавати.
REM Такого не робить жоден інструмент ринку.
REM
REM ЧОМУ 6 ГОДИН І ЗІ ЗСУВОМ. Джерело даних — capital-groups.json, який оновлює
REM groupsig раз на 6 годин. Питати частіше немає сенсу: відповідь не зміниться.
REM Зсув на 40 хвилин потрібен, щоб не читати файл у момент, коли groupsig його пише.
REM
REM ПАУЗА ЧЕРЕЗ Start-Sleep, А НЕ `timeout`: `timeout /t` падає при перенаправленому
REM вводі (запуск іде з .vbs із виводом у файл), і цикл крутиться всуху.
REM
REM Кожен алерт цього біна одразу пишеться в журнал сигналів, тож через 1/7/30/90 днів
REM sigcheck порахує, чи він справдився. Монітор народжується вже перевірюваним.
REM ─────────────────────────────────────────────────────────────────────────────
set "ROOT=C:\Users\StarOsta\wallet-tracker-CODE-backup-2026-09-08"
set "LOG=D:\wr-live\groupwatch.log"
set "INTERVAL_SEC=21600"

set "DATASET_DIR=D:\wallet-radar-backup\current\dataset"

REM Поріг істотної зміни частки емісії, у процентних пунктах. Нижче — шум.
set "MIN_DELTA_PP=0.25"
REM Позиції, дрібніші за це, алертів не породжують.
set "MIN_USD=50000"
set "TOP=12"

REM Сховище довідників: без цих змінних біни падають із поясненням.
REM Слеші ПРЯМІ: зворотні з'їдаються при читанні оболонкою.
set "WR_DB=D:/wr-live/store/wallet-radar.db"
set "WR_SEED=D:/wr-live/store/seed"
set "WR_ARCHIVE_ROOTS=D:/wallet-radar-backup/current"
set "WR_REPO=C:/Users/StarOsta/wallet-tracker-CODE-backup-2026-09-08"

cd /d "%ROOT%"

REM Зсув від groupsig: даємо йому дописати capital-groups.json.
powershell -NoProfile -Command "Start-Sleep -Seconds 2400"

:loop
if not exist "D:\wr-live" (
  echo [%date% %time%] диска D: немає - чекаю >> "%ROOT%\groupwatch-fallback.log"
  powershell -NoProfile -Command "Start-Sleep -Seconds 300"
  goto loop
)

tasklist /fi "imagename eq groupwatch.exe" 2>nul | find /i "groupwatch.exe" >nul
if not errorlevel 1 (
  echo [%date% %time%] попереднiй прогiн ще йде - пропускаю >> "%LOG%"
  powershell -NoProfile -Command "Start-Sleep -Seconds 600"
  goto loop
)

echo [%date% %time%] === прогiн === >> "%LOG%"
"%ROOT%\backfill\target\release\groupwatch.exe" "%ROOT%" >> "%LOG%" 2>&1
if errorlevel 1 echo [%date% %time%] ПОМИЛКА, код %errorlevel% >> "%LOG%"

REM -- ПАУЗА ЗА СТІННИМ ГОДИННИКОМ (виправлено 26.09.2026) --
REM `Start-Sleep -Seconds N` не рахує час, коли система СПАЛА. Цей ноут засинає по
REM кілька разів на добу, тому пауза «на 6 годин» тривала 33 години реального часу:
REM цикл виглядав налаштованим, а дані старіли. Тепер чекаємо до ЦІЛЬОВОГО ЧАСУ
REM короткими кроками — після пробудження годинник уже за межею, і коло йде далі.
powershell -NoProfile -Command "$t=(Get-Date).AddSeconds(%INTERVAL_SEC%); while((Get-Date) -lt $t){ Start-Sleep -Seconds 30 }"
goto loop
