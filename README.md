# Wallet Radar

> **Про цей репозиторій.** Тут лежить **рушій без даних**: код аналізу, але не
> результати. Свідомо НЕ включено:
> - `dataset/` — зібрані дані досліджень (десятки тисяч адрес);
> - `server/` — легасі Node-бекенд, який витісняється (джерело правди — Rust);
> - golden-фікстури тестів — це знімки реальних даних;
> - внутрішні звіти, нотатки сесій і runbook інфраструктури.
>
> Довідники (адреси бірж, пороги, групи, параметри стратегій) живуть у SQLite
> (крейт `wr-store`), а не в коді — див. `wr-store/migrations/`. Порожня база
> означає, що застосунок **не стартує** з поясненням, а не працює на нулях.
> Наповнити: `wr-import extract && wr-import load`.

Мультичейн-система розвідки крипто-гаманців: баланси й активи в реальному часі, детекція
китів і координованих рухів, граф зв'язків, бектести, ML-моделі «розумних грошей», Telegram-алерти.

> **Головне про архітектуру:** прод — це **Rust**. Бекенд, бекфіл і всі фонові аналітичні
> задачі виконує Rust. Легасі Node-сервер лишається в дереві **лише як еталон парності** для
> `parity_check` і в проді не запускається. Дет��лі — у розділі [Мовний склад](#мовний-склад).

---

## Що це робить

Відстежує гаманці в багатьох мережах: баланси, портфель із графіком, живу стрічку подій,
граф зв'язків («павутина»), Tracer (трасування потоків), Alerts (правила сповіщень),
Entity-профілі, настроюваний Dashboard, пошук по активах і адресах усіх мереж. Інтерфейс — 5 мов.

Ключова риса — **детекція «десків» і капітал-груп**: пошук координованих китів, злиття
під-мереж, класифікація архетипів, помісячна активність, розвідсторінка з живим force-графом
(`/network.html`).

---

## Архітектура

```
                         ┌─────────────────────────────┐
     браузер  ◄── :3000 ─┤   rust-server (axum/tokio)   │  ← ПРОД, systemd
     web/ (TS→JS)        │   89 API-маршрутів + WS      │    wallet-radar-rust.service
                         │   + 20 фонових воркерів      │
                         └──────────────┬──────────────┘
                                        │ gRPC + mTLS (tonic)
                                        │ у кора НЕМАЄ жодного ключа
                         ┌──────────────▼──────────────┐
                         │  satellite × N               │  ← тримають API-ключі,
                         │  etherscan/moralis/arkham/…  │    підставляють їх до провайдера
                         └─────────────────────────────┘
```

**Модель безпеки (свідома):** кор не тримає ключів. Кожен виклик платного/keyed API йде по
gRPC поверх mTLS до сателіта, який підставляє ключ уже зі свого оточення. `Fetch` у
`proto/satellite.proto` навмисно загальний — сателіт повертає тіло провайдера **байт-у-байт**,
тож golden-перевірені парсери кора лишаються недоторканими. Провайдер лише з allowlist —
довільний хост передати неможливо (захист від SSRF).

---

## Мовний склад

Що на чому написано і що з цього **реально виконується в проді**:

| Компонент | Мова | Прод? | Роль |
|---|---|---|---|
| `rust-server/` | **Rust** | ✅ так | Бекенд :3000 — API, WS, 20 воркерів. Головний процес. |
| `backfill/` | **Rust** | ✅ так | Бекфіл в Elasticsearch (`backfill2`) + біни `relay-monitor`, `supply`, `eth-*`, `btc-*`, `distrib-alert`… |
| `nextaction/` | **Rust** | ✅ так | ~60 бінів аналітики: `strategy`, `volatility`, `sector`, `rotation`, `pnl`, `cohort`, `capgroups`, `rg*`, `train`, `merge-graph`… |
| `features/` | **Rust** | ✅ так | Витяг ознак, кластеризація родин, `model`. |
| `satellite/` | **Rust** | ✅ так | Key-holding проксі (gRPC/mTLS). |
| `gat/` | **Rust** | ⏸ офлайн | Graph Attention Network (ML), окремий Dockerfile. |
| `web/` (`src/*.ts`) | **TypeScript** | ✅ так | Фронтенд. Компілюється в браузерний `app.js` (esbuild) — **на Rust бути не може за визначенням**. |
| `deploy/*.sh`, `diskguard.sh` | **Shell** | ✅ так | Клей крону: планують і запускають Rust-біни. |
| `deploy/group-monitor.py` | **Python** | ⚠️ **так** | **Єдиний функціональний не-Rust у проді.** Щоденний Telegram-дайджест по 2 групах. Кандидат на порт (див. нижче). |
| `server/*.js`, `*.mjs` | **Node** | ❌ **ні** | Легасі-реалізація. **Не запускається.** Еталон для `parity_check`. Не видаляти. |
| `scripts/`, `tools/gen_*` | **Node** | 🔧 build | Допоміжні: `stamp-assets.mjs`, генератори golden-фікстур. |
| `test/*.test.ts` | **TypeScript** | 🧪 CI | Тести через `node --test`. |

---

## Статус міграції на Rust

Порт із Node на Rust **фактично завершено**:

- **Rust віддає 89 API-маршрутів — надмножина 70 маршрутів легасі-Node.** Функціонал не
  втрачено, у Rust його більше.
- **`parity_check`** (бін у `rust-server/src/bin/`) звіряє ключові маршрути **байт-у-байт**
  між Node і Rust. Це запобіжник: поки він зелений, Rust гарантовано не розійшовся з
  перевіреною поведінкою попередніх ітерацій. Тому `server/` тримають живим — як референс.
- Навіть Python-версію парність-чекера вже портовано в Rust (`tools/parity_check.py` прибрано).

**Що ще НЕ на Rust і чому:**

1. **`deploy/group-monitor.py`** — єдиний не-Rust runtime. Робить інкрементальний краул ETH
   ERC-20 по ядрах 2 груп і шле Telegram-дельту. Порт реальний: у Rust уже є перекривні біни
   (`netflow`, `emission`, `distrib-alert`) і `telegram.rs`. ⚠️ Додатковий мотив: скрипт читає
   `.env` **напряму** — тобто обходить модель «кор без ключів». Порт у Rust-бін, що ходить
   через сателіт, закрив би і це.
2. **Фронтенд (`web/src`)** — браузерний код, на Rust не переноситься (хіба WASM, що недоцільно).
3. **Shell-клей** — доречний як планувальник; переписувати в Rust сенсу мало.
4. **Node `server/` + `.mjs`** — навмисно лишаються як еталон парності. Видалення = втрата
   запобіжника проти регресій, тому НЕ чіпаємо.

---

## Запуск

### Прод (як воно працює на сервері)

Бекенд — systemd, не Docker і не `npm start`:

```bash
# зібрати реліз
cd rust-server && cargo build --release

# керування сервісом
systemctl status  wallet-radar-rust.service      # головний бекенд :3000
systemctl restart wallet-radar-rust.service
journalctl -u wallet-radar-rust.service -f       # логи

# фонові Rust-задачі — по systemd-таймерах і crontab (deploy/*.sh)
systemctl list-timers 'wallet-*'
crontab -l
```

### Фронтенд

Джерело — `web/src/*.ts` + `web/tw-input.css`. Перед подачею зібрати:

```bash
npm install
npm run build     # typecheck (tsc) + esbuild (web/src/main.ts → web/app.js) + tailwind + stamp-assets
```

Rust-сервер віддає статику `web/` та API з одного порту `:3000`.

### Локальний Docker (легасі, довідково)

`docker-compose.yml` збирає **стару Node-збірку** — це не прод. Піднятий без сателітів,
працює на безключових джерелах. Для відтворення прод-поведінки використовуйте Rust + сателіти.

---

## Конфігурація та секрети

- **`.env`** (права `600`, у `.gitignore`) — лише власні секрети кора: паролі,
  `TELEGRAM_BOT_TOKEN`, `SENTRY_DSN`, `ADMIN_PASSWORD`/`APP_PASSWORD`.
- **Ключів до Etherscan/Moralis/Arkham/Glassnode/LunarCrush у `.env` кора НЕМАЄ** — їх тримають
  сателіти (`deploy/docker-compose.satellites.yml` + `deploy/.env`, див. `docs/satellites.md`).
- **mTLS-PKI** сателітів — `deploy/pki/out/` (у `.gitignore`).

---

## API

- Специфікація: `rust-server/src/openapi_spec.json` (89 маршрутів).
- Базові: `GET /api/config`, `/api/wallets` (+`POST`/`DELETE`), `/api/events?limit=N`,
  `/api/whale-assets`, `/api/capital-groups`, `/api/cross-asset`, `/api/asset-forensics`,
  `/api/meta-subnets`, WS `ws://…/ws`.
- Сервер за Basic-авторизацією (пароль з `ADMIN_PASSWORD`/`APP_PASSWORD`); у локальному режимі
  без пароля пускає.

---

## Структура

```
rust-server/       ПРОД. axum-бекенд, 75 модулів + bin/parity_check
  src/openapi_spec.json    специфікація API
backfill/          Rust: бекфіл в Elasticsearch + ~15 бінів (relay-monitor, supply, eth-*, btc-*)
nextaction/        Rust: ~60 бінів аналітики (strategy, volatility, capgroups, rg*, train, cohort…)
features/          Rust: витяг ознак, кластеризація родин, model
satellite/         Rust: key-holding проксі (gRPC/mTLS)
gat/               Rust: Graph Attention Network (офлайн-ML)
proto/             satellite.proto — контракт кор↔сателіт
web/               Фронтенд: src/*.ts → app.js (esbuild) + app.css (tailwind); index.html, network.html
dataset/           Живі датасети/моделі (JSON/CSV) — версіонуються в git навмисно
deploy/            Крон-скрипти (shell) + group-monitor.py + PKI + satellites-compose
docs/              satellites.md, brandbook.html
server/            ЛЕГАСІ Node — НЕ запускається; еталон для parity_check
test/              node --test (*.test.ts)
tools/, scripts/   Build-хелпери (Node): генератори фікстур, stamp-assets
old/               Архів прибраних dev-артефактів (у .gitignore)
.env               Секрети кора (600, не комітиться)
```

---

## Тестування

```bash
npm test                                    # node --test: golden-парність парсерів
cd rust-server && cargo test                # Rust-юніти
# парність Node↔Rust: Node на 3000, Rust на 3001
cargo run --release --bin parity_check
```

Golden-фікстури генерують `tools/gen_*_fixtures.mjs`; вони фіксують еталонний вихід парсерів,
проти якого звіряються обидві реалізації.
