@echo off
REM ─────────────────────────────────────────────────────────────────────────────
REM Ставить Telegram-бот у автозапуск Windows (Планувальник завдань).
REM
REM Виклик: install-task.cmd
REM Після встановлення бот піднімається разом із системою й пише лог у
REM deploy\botd.log.
REM ─────────────────────────────────────────────────────────────────────────────
setlocal
set "TASK=WalletRadarTelegramBot"
set "RUNNER=%~dp0run-bot.cmd"

schtasks /Delete /TN "%TASK%" /F >nul 2>&1
schtasks /Create /TN "%TASK%" /TR "\"%RUNNER%\"" /SC ONSTART /RU SYSTEM /RL HIGHEST /F
if errorlevel 1 (
    echo [botd] НЕ вдалося створити завдання %TASK%
    exit /b 1
)
echo [botd] Завдання %TASK% створено: бот підніметься разом із системою.
echo [botd] Перевірити журнал: schtasks /Query /TN "%TASK%" /V /FO LIST
echo [botd] Запустити просто зараз: schtasks /Run /TN "%TASK%"
endlocal
