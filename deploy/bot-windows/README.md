# Telegram-бот на Windows-машині — запуск і перемикання

Бот (`backfill/src/botd.rs`) працює **на одній машині з даними**, але реєстр
підключених живе у спільному місці — на сервері Wallet Radar:

- бот пише реєстр → `POST http://<сервер>:3000/api/telegram-users` (токен `TELEGRAM_SYNC_TOKEN`);
- сервер читає цей реєстр і розсилає свої алерти тим самим людям.

## Головне правило: один поллер на токен

Telegram не підтримує двох `getUpdates` з одним токеном — другий отримує
`409 Conflict`. Практично це виглядає так: у лозі «опитування не вдалось,
чекаю 320с», а один із двох ботів не отримує жодної команди.

Тому запускати **можна тільки один** — або на Windows, або на Linux-ноутбуці.

### Перемикання на Windows

1. Зупинити бот на Linux-ноутбуці:

   ```bash
   systemctl stop wallet-radar-bot.service
   ```

2. На Windows оновити код (реєстр і .env — у цій самій копії):

   ```powershell
   cd C:\Users\StarOsta\wallet-tracker-CODE-backup-2026-09-08
   git fetch origin
   git checkout local/work-snapshot-2026-11-07
   git pull
   ```

   Якщо Git-репозиторію на Windows немає — скопіювати гілку
   `local/work-snapshot-2026-11-07` з Gitea (`http://<ноутбук>:3030/vitalii/wallet-tracker`)
   або з диска 1 ТБ (`wallet-radar-backup\current`).

3. Зібрати бота:

   ```powershell
   cargo build --release --bin botd
   ```

   Потрібен Rust і кеш cargo; перша збірка — кілька хвилин.

4. Перевірити `.env` у корені копії:

   | ключ | значення | навіщо |
   |---|---|---|
   | `TELEGRAM_BOT_TOKEN` | токен бота | обов'язковий |
   | `TELEGRAM_SYNC_TOKEN` | той самий, що на сервері | запис реєстру в спільне місце |
   | `TELEGRAM_USERNAME` | `vitalii_vedana` | власник = адмін |
   | `TELEGRAM_ADMINS` | необов'язково | додаткові адміни через кому |
   | `TELEGRAM_ACCESS_CODE` | необов'язково | код підключення; порожньо = доступ усім |
   | `ADMIN_PASSWORD` / `APP_PASSWORD` | паролі цієї копії | `/api/ask` від імені глядача |

5. Встановити автозапуск:

   ```cmd
   deploy\bot-windows\install-task.cmd
   ```

   або вручну: `schtasks /Run /TN "WalletRadarTelegramBot"`.

6. Перевірити:

   ```cmd
   type deploy\botd.log
   curl http://192.168.2.112:3000/api/telegram-status
   ```

   У лозі має з'явитися `[botd] слухаю команди від усіх підключених · адміни: @vitalii_vedana`
   і раз на хвилину `[botd] реєстр синхронізовано із сервером`.

## Старий сервер на Windows і конфлікт опитування

Windows-машина має свій `rust-server` (пише в `wr-live\`). Він теж вміє опитувати
Telegram, і разом із ботом це дає `409`. Щоб цього не було:

- або зупинити старий сервер на Windows (канонічний сервер — на Linux-ноутбуці);
- або оновити `rust-server/src/telegram.rs` до версії, де `ready()` враховує
  реєстр: тоді сервер не опитує Telegram, поки реєстр не порожній
  (гілка `local/work-snapshot-2026-11-07` уже містить цю правку).

Поки бот не запущений на Windows, а на Linux-ноутбуці — реєстр на сервері
живий сам по собі: алерти сервера йдуть усім, хто в ньому записаний.

## Якщо бот усе одно пише «409»

Багато поллерів: перевір, чи не працює другий екземпляр (на іншій машині,
або забутий таймер Windows: `schtasks /Query /TN "WalletRadarTelegramBot"`).
