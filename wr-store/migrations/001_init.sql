-- wr-store 001 — початкова схема.
-- Мета: з .rs зникають адреси, пороги, групи, стратегії та реєстр ключів.
-- Значення секретів сюди НЕ пишуться: у базі лише те, ЯК їх знайти
-- (Windows Credential Manager). Інакше секрети поїхали б у кожен бекап файлу БД.

-- PRAGMA тут НЕМАЄ навмисно: міграції накочуються під транзакцією, а journal_mode
-- у транзакції змінити не можна. Обидві прагми — властивість з'єднання, тож їх
-- ставить lib.rs при відкритті.

-- ── мережі ──────────────────────────────────────────────────────────────────
CREATE TABLE chains (
  id             TEXT PRIMARY KEY,        -- 'eth', 'arbitrum', 'base', ...
  name           TEXT NOT NULL,
  native_symbol  TEXT NOT NULL,
  explorer_base  TEXT,
  enabled        INTEGER NOT NULL DEFAULT 1
) STRICT;

-- ── константи мережі та пороги рушія ────────────────────────────────────────
-- Сюди переїжджає весь wr-dict: EXCHANGE_TX, HUB_PCT, MIN_CLUSTER, GRAB_EMISSION,
-- WHALE_BALANCE_ANOMALY_ETH, топік Transfer, zero-address тощо.
-- kind каже, як читати value: 'int' | 'float' | 'hex' | 'text'.
CREATE TABLE constants (
  key          TEXT PRIMARY KEY,
  value        TEXT NOT NULL,
  kind         TEXT NOT NULL CHECK (kind IN ('int','float','hex','text')),
  unit         TEXT,
  description  TEXT NOT NULL,
  updated_at   TEXT NOT NULL DEFAULT (datetime('now'))
) STRICT;

-- ── токени: каталог контрактів ──────────────────────────────────────────────
-- Замінює BASE_JSON з universe.rs, real_money() з wr-dict і списки стейблів
-- у dexswap.rs / behavfp.rs. money_kind != NULL означає «справжні гроші»
-- для P&L (Usd | Eth | Matic) — спам-двійники сюди не потрапляють.
CREATE TABLE tokens (
  chain        TEXT NOT NULL REFERENCES chains(id),
  contract     TEXT NOT NULL,             -- lowercase, з 0x
  symbol       TEXT NOT NULL,
  name         TEXT,
  -- nullable НАВМИСНО: у частини джерел десяткових немає. NULL = «невідомо»,
  -- і код мусить про це спіткнутись. Підставлене 18 за замовчуванням тихо
  -- зламало б USDC/USDT (у них 6) — помилка, яку неможливо помітити в логах.
  decimals     INTEGER,
  sector       TEXT,
  money_kind   TEXT CHECK (money_kind IN ('usd','eth','matic')),
  cg_id        TEXT,                      -- ідентифікатор CoinGecko
  source       TEXT NOT NULL,             -- 'universe.rs', 'wr-dict', 'defillama', ...
  verified     INTEGER NOT NULL DEFAULT 0,
  updated_at   TEXT NOT NULL DEFAULT (datetime('now')),
  PRIMARY KEY (chain, contract),
  CHECK (contract = lower(contract))
) STRICT;
CREATE INDEX tokens_symbol ON tokens(symbol);

-- Активний всесвіт — те, що universe.rs перезаписує щодня зі свіжими капіталізаціями.
-- Окремо від каталогу, бо це ЗМІННИЙ зріз, а не властивість токена.
CREATE TABLE universe (
  chain      TEXT NOT NULL,
  contract   TEXT NOT NULL,
  size       REAL NOT NULL,
  position   INTEGER NOT NULL,
  as_of      TEXT NOT NULL DEFAULT (datetime('now')),
  PRIMARY KEY (chain, contract),
  FOREIGN KEY (chain, contract) REFERENCES tokens(chain, contract)
) STRICT;

-- ── гаманці ─────────────────────────────────────────────────────────────────
-- Сюди переїжджають 140 адрес із 25 файлів: мітки бірж (entities.rs),
-- гаманці-предмети дослідження (cexsignal.rs, aftermoney.rs, relay_monitor.rs),
-- хаби, мости, пули.
CREATE TABLE wallets (
  chain        TEXT NOT NULL REFERENCES chains(id),
  address      TEXT NOT NULL,
  -- перелік описує те, що РЕАЛЬНО є в коді (entities.rs має defi/issuer/unknown),
  -- а не те, як гарно було б. Звужений CHECK змусив би імпорт перекручувати дані.
  kind         TEXT NOT NULL CHECK (kind IN
                 ('cex','bridge','contract','pool','router','eoa',
                  'defi','issuer','burn','unknown')),
  label        TEXT,                      -- 'Binance 14', 'Coinbase', ...
  entity       TEXT,                      -- 'Binance' — власник, спільний для міток
  is_contract  INTEGER,                   -- NULL = не перевіряли; eth_getCode
  source       TEXT NOT NULL,
  first_seen   TEXT,
  notes        TEXT,
  updated_at   TEXT NOT NULL DEFAULT (datetime('now')),
  PRIMARY KEY (chain, address),
  CHECK (address = lower(address))
) STRICT;
CREATE INDEX wallets_kind   ON wallets(kind);
CREATE INDEX wallets_entity ON wallets(entity);

-- Роль гаманця в дослідженні. Одна адреса може мати кілька ролей у різних контекстах,
-- тому ctx — це, наприклад, ідентифікатор групи або назва розслідування.
CREATE TABLE wallet_roles (
  chain    TEXT NOT NULL,
  address  TEXT NOT NULL,
  role     TEXT NOT NULL CHECK (role IN
             ('seed','concentrator','distributor','core','counterparty',
              'paymaster','treasury','hub','victim','mask')),
  ctx      TEXT NOT NULL DEFAULT '',
  since    TEXT,
  notes    TEXT,
  PRIMARY KEY (chain, address, role, ctx),
  FOREIGN KEY (chain, address) REFERENCES wallets(chain, address) ON DELETE CASCADE
) STRICT;

-- ── групи капіталу ──────────────────────────────────────────────────────────
CREATE TABLE groups (
  id          TEXT PRIMARY KEY,           -- 'NET', 'HUB', 'INFRA', 'CG07', ...
  name        TEXT NOT NULL,
  kind        TEXT NOT NULL CHECK (kind IN
                ('capital','partition','bucket','investigation')),
  status      TEXT NOT NULL DEFAULT 'active'
                CHECK (status IN ('active','archived','candidate')),
  monitored   INTEGER NOT NULL DEFAULT 0,
  created_at  TEXT NOT NULL DEFAULT (datetime('now')),
  notes       TEXT
) STRICT;

CREATE TABLE group_members (
  group_id    TEXT NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
  chain       TEXT NOT NULL,
  address     TEXT NOT NULL,
  role        TEXT,
  confidence  REAL,                       -- 0..1; слабкий зв'язок vs підтверджений
  joined_via  TEXT,                       -- 'edges', 'portfolio-idf', 'seed', ...
  PRIMARY KEY (group_id, chain, address),
  FOREIGN KEY (chain, address) REFERENCES wallets(chain, address) ON DELETE CASCADE
) STRICT;
CREATE INDEX group_members_addr ON group_members(chain, address);

-- Аналітика груп. Зберігається З ДАТОЮ: це часовий ряд, а не одне поточне число,
-- інакше неможливо показати, що оцінка капіталу впала $1.59млрд → $2.82млн і чому.
CREATE TABLE group_metrics (
  group_id   TEXT NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
  metric     TEXT NOT NULL,               -- 'real_capital_usd', 'funded_usd', 'risk_score', ...
  as_of      TEXT NOT NULL,
  value_num  REAL,
  value_text TEXT,
  method     TEXT NOT NULL,               -- 'capreal', 'flow-net', 'ethbal' — ЯК пораховано
  source     TEXT,
  PRIMARY KEY (group_id, metric, as_of, method)
) STRICT;

CREATE TABLE group_assets (
  group_id     TEXT NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
  chain        TEXT NOT NULL,
  contract     TEXT NOT NULL,
  emission_pct REAL,
  usd_value    REAL,
  as_of        TEXT NOT NULL,
  PRIMARY KEY (group_id, chain, contract, as_of),
  FOREIGN KEY (chain, contract) REFERENCES tokens(chain, contract)
) STRICT;

-- ── стратегії ───────────────────────────────────────────────────────────────
CREATE TABLE strategies (
  id          TEXT PRIMARY KEY,           -- 'selltape', 'cascade', 'rgpredict', ...
  name        TEXT NOT NULL,
  kind        TEXT NOT NULL,
  enabled     INTEGER NOT NULL DEFAULT 1,
  version     TEXT NOT NULL DEFAULT '1',
  description TEXT,
  created_at  TEXT NOT NULL DEFAULT (datetime('now'))
) STRICT;

-- Пороги стратегій: те, що зараз розірване між константами wr-dict
-- і ~200 env-змінними (MULT, MIN_USD, SELL_PCT, COOLDOWN_MIN, MIN_EMIS_PCT ...).
CREATE TABLE strategy_params (
  strategy_id TEXT NOT NULL REFERENCES strategies(id) ON DELETE CASCADE,
  key         TEXT NOT NULL,
  value       TEXT NOT NULL,
  kind        TEXT NOT NULL CHECK (kind IN ('int','float','bool','text','json')),
  unit        TEXT,
  description TEXT NOT NULL,
  PRIMARY KEY (strategy_id, key)
) STRICT;

-- ── ключі: РЕЄСТР, не значення ──────────────────────────────────────────────
-- Значення живе в Windows Credential Manager під (service, account).
-- Тут лише те, що система мусить знати, щоб голосно сказати «ключа немає»
-- на старті, а не тихо працювати впівсили (як було, коли переїзд env-файлів
-- на тиждень поклав cexsignal).
CREATE TABLE secrets (
  name        TEXT PRIMARY KEY,           -- 'ETHERSCAN_API_KEY', 'TELEGRAM_BOT_TOKEN', ...
  service     TEXT NOT NULL,              -- ключ Credential Manager
  account     TEXT NOT NULL,
  required    INTEGER NOT NULL DEFAULT 1,
  env_fallback TEXT,                      -- ім'я env-змінної для headless Linux/ВМ
  description TEXT NOT NULL
) STRICT;

-- Ключі з квотою і ротацією (Etherscan). Значення — теж у Credential Manager,
-- знайдене за secrets.name; тут лише облік, бо 3 запити/с і 100k/добу
-- треба рахувати, а ретрай теж списується.
CREATE TABLE api_keys (
  key_id      TEXT PRIMARY KEY,
  provider    TEXT NOT NULL,              -- 'etherscan', 'moralis', ...
  secret_name TEXT NOT NULL REFERENCES secrets(name),
  rps         REAL,
  daily_quota INTEGER,
  enabled     INTEGER NOT NULL DEFAULT 1,
  notes       TEXT
) STRICT;

CREATE TABLE api_key_usage (
  key_id    TEXT NOT NULL REFERENCES api_keys(key_id) ON DELETE CASCADE,
  day       TEXT NOT NULL,
  requests  INTEGER NOT NULL DEFAULT 0,
  errors    INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (key_id, day)
) STRICT;
