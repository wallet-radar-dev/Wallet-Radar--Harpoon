-- wr-store 003 — символ токена може бути невідомим, і це треба вміти сказати.
--
-- Було: `symbol TEXT NOT NULL`, тож імпортер, натрапивши на голий перелік
-- контрактів без символів, підставляв `'?'`. Рядок `'?'` — не «невідомо»,
-- а стверджувальна брехня: код, що шукає токен за символом, знайде «?»
-- і повірить. Той самий урок, що з `decimals`: NULL чесніший за заглушку.
--
-- SQLite не вміє знімати NOT NULL через ALTER, тож перебудова таблиці.
--
-- ВАЖЛИВО ПРО PRAGMA. Перша версія починалась із `PRAGMA foreign_keys = OFF`
-- і не працювала: ця прагма — НЕ-ОПЕРАЦІЯ всередині транзакції, а міграції
-- накочуються транзакцією. `DROP TABLE tokens` наштовхувався на посилання
-- з token_tags / universe / group_assets і падав, міграція відкочувалась.
-- (Той самий клас помилки, що й `journal_mode` у 001.)
-- Працює `defer_foreign_keys`: він відкладає перевірку до коміту, тобто
-- дає перебудувати таблицю, але НЕ дозволяє лишити висячі посилання.

PRAGMA defer_foreign_keys = ON;

-- ── 1. Помилка класифікації: гаманці, що потрапили в tokens ────────────────
--
-- Завантажувач вирішував «токен чи гаманець» за КІЛЬКІСТЮ полів кортежу:
-- пара (адреса, підпис) вважалась токеном, трійка — гаманцем. Через це
-- `const INFRA: [(&str, &str); 10]` із cexsignal.rs — концентратор мережі,
-- дистриб'ютор і п'ять реле — опинились у таблиці токенів, а їхні описи
-- («🏦 КОНЦЕНТРАТОР…») стали «символами».
--
-- Усі 10 адрес паралельно є в `wallets`, тож дані не втрачено. Прибираємо
-- спершу теги, потім самі рядки — інакше лишились би висячі посилання.
DELETE FROM token_tags
 WHERE contract IN (
   SELECT t.contract FROM tokens t
    JOIN wallets w ON w.chain = t.chain AND w.address = t.contract
   WHERE t.source LIKE '%cexsignal.rs'
 );

DELETE FROM tokens
 WHERE source LIKE '%cexsignal.rs'
   AND contract IN (SELECT address FROM wallets WHERE chain = tokens.chain);

-- ── 2. Перебудова з nullable symbol ────────────────────────────────────────
CREATE TABLE tokens_new (
  chain        TEXT NOT NULL REFERENCES chains(id),
  contract     TEXT NOT NULL,
  symbol       TEXT,                      -- NULL = символ невідомий
  name         TEXT,
  decimals     INTEGER,                   -- NULL = невідомо (див. 001)
  sector       TEXT,
  money_kind   TEXT CHECK (money_kind IN ('usd','eth','matic')),
  cg_id        TEXT,
  source       TEXT NOT NULL,
  verified     INTEGER NOT NULL DEFAULT 0,
  updated_at   TEXT NOT NULL DEFAULT (datetime('now')),
  PRIMARY KEY (chain, contract),
  CHECK (contract = lower(contract))
) STRICT;

-- '?' був заглушкою — переносимо його як NULL
INSERT INTO tokens_new
SELECT chain, contract, NULLIF(symbol, '?'), name, decimals, sector,
       money_kind, cg_id, source, verified, updated_at
FROM tokens;

DROP TABLE tokens;
ALTER TABLE tokens_new RENAME TO tokens;
CREATE INDEX tokens_symbol ON tokens(symbol);
