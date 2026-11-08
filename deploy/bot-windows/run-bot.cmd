@echo off
REM ─────────────────────────────────────────────────────────────────────────────
REM Telegram-бот Wallet Radar — запуск на Windows-машині.
REM
REM Бот має бути ЄДИНИМ, хто опитує Telegram цим токеном. Якщо бот уже працює
REM на Linux-ноутбуці (wallet-radar-bot.service), спершу зупини його:
REM     systemctl stop wallet-radar-bot.service
REM Інакше Telegram відповідатиме 409 Conflict ("terminated by other getUpdates")
REM і бот почне сипати помилками опитування.
REM
REM Код і .env беруться з кореня копії (APP_ROOT), реєстр — deploy\telegram-users.json,
REM синхронізація реєстру — на сервер (API_HOST:API_PORT).
REM ─────────────────────────────────────────────────────────────────────────────
setlocal
set "ROOT=%~dp0..\.."
for %%I in ("%ROOT%") do set "ROOT=%%~fI"

set "APP_ROOT=%ROOT%"
REM Сервер Wallet Radar — спільне місце для реєстру й алертів.
set "API_HOST=192.168.2.112"
set "API_PORT=3000"

REM Реєстр має існувати до старту: інакше бот почне з порожнього і перезапише
REM серверний реєстр порожнечим.
if not exist "%ROOT%\deploy\telegram-users.json" (
    echo {"users":{}}>"%ROOT%\deploy\telegram-users.json"
)

if not exist "%ROOT%\backfill\target\release\botd.exe" (
    echo [botd] НЕМА botd.exe — збери його: cargo build --release --bin botd
    exit /b 1
)

"%ROOT%\backfill\target\release\botd.exe" "%ROOT%" >> "%ROOT%\deploy\botd.log" 2>&1
endlocal
