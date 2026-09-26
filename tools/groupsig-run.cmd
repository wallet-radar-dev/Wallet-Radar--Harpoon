@echo off
REM ─────────────────────────────────────────────────────────────────────────────
REM groupsig — ЄДИНЕ, що оновлює відсотки емісії у стратегіях.
REM
REM Поширена помилка: думати, що дані груп оновлює воркер whale-finder. Ні — той пише
REM whale-finder.json і whale-clusters.json. capital-groups.json, звідки застосунок
REM бере emissionShare, пише саме groupsig: він тягне РЕАЛЬНІ баланси значущих позицій
REM і total supply з Etherscan.
REM
REM Бін обробляє ОДНУ групу за прогін, тому тут перелік. Значущі групи — ті, що мають
REM tokenSignals: NET, NET-WEAK, HUB. Ключі бін знаходить сам: оточення ->
REM deploy\backfill.env -> .env.
REM
REM ПАУЗА ЧЕРЕЗ Start-Sleep, А НЕ `timeout`: `timeout /t` не працює при перенаправленому
REM вводі (а запуск іде з .vbs із виводом у файл), падає миттєво — і цикл крутиться
REM всуху. Через це 14.09 бін прокрутився 171 раз замість чотирьох і спалив ~90 тис.
REM зайвих запитів Etherscan.
REM ─────────────────────────────────────────────────────────────────────────────
set "ROOT=C:\Users\StarOsta\wallet-tracker-CODE-backup-2026-09-08"
set "LOG=D:\wr-live\groupsig.log"
set "INTERVAL_SEC=21600"

REM ── СХОВИЩЕ ДОВІДНИКІВ ────────────────────────────────────────────────────
REM Після переїзду адрес/порогів/стратегій у SQLite бінарники БЕЗ цих змінних
REM не стартують — падають із поясненням, а не працюють впівсили. Дублюємо тут
REM явно (як і DATASET_DIR): якщо deploy\store.env колись перезапишуть, запуск
REM однаково лишиться правильним.
REM Слеші ПРЯМІ: зворотні з'їдаються при читанні оболонкою.
set "WR_DB=D:/wr-live/store/wallet-radar.db"
set "WR_SEED=D:/wr-live/store/seed"
set "WR_ARCHIVE_ROOTS=D:/wallet-radar-backup/current"
set "WR_REPO=C:/Users/StarOsta/wallet-tracker-CODE-backup-2026-09-08"


cd /d "%ROOT%"

:loop
if not exist "D:\wr-live" (
  echo [%date% %time%] диска D: немає - чекаю >> "%ROOT%\groupsig-fallback.log"
  powershell -NoProfile -Command "Start-Sleep -Seconds 300"
  goto loop
)

REM Другий екземпляр писав би в той самий capital-groups.json одночасно з першим.
tasklist /fi "imagename eq groupsig.exe" 2>nul | find /i "groupsig.exe" >nul
if not errorlevel 1 (
  echo [%date% %time%] попереднє коло ще йде - пропускаю >> "%LOG%"
  powershell -NoProfile -Command "Start-Sleep -Seconds 600"
  goto loop
)

for %%G in (NET NET-WEAK HUB) do (
  echo [%date% %time%] === %%G === >> "%LOG%"
  "%ROOT%\backfill\target\release\groupsig.exe" "%ROOT%" %%G >> "%LOG%" 2>&1
  if errorlevel 1 echo [%date% %time%] %%G: ПОМИЛКА, код %errorlevel% >> "%LOG%"
)
echo [%date% %time%] коло завершено, наступне через %INTERVAL_SEC% с >> "%LOG%"

REM ── ПАУЗА ЗА СТІННИМ ГОДИННИКОМ (виправлено 26.09.2026) ──
REM `Start-Sleep -Seconds N` не рахує час, коли система СПАЛА. Цей ноут засинає по
REM кілька разів на добу, тому пауза «на 6 годин» тривала 33 години реального часу:
REM цикл виглядав налаштованим, а capital-groups.json старів. Тепер чекаємо до
REM ЦІЛЬОВОГО ЧАСУ короткими кроками — після пробудження годинник уже за межею,
REM і коло йде далі без затримки.
powershell -NoProfile -Command "$t=(Get-Date).AddSeconds(%INTERVAL_SEC%); while((Get-Date) -lt $t){ Start-Sleep -Seconds 30 }"
goto loop
