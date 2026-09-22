-- wr-store 002 — членство в іменованих списках.
--
-- Останні 19 адрес у коді виявились не таблицями з мітками, а ПЕРЕЛІКАМИ:
-- `const KNOWN_CEX`, `const PARIBU`, `const ETH_TOKENS`, `const DISPENSERS`…
-- Для них не вистачало жодної наявної таблиці: `wallets.kind` — це властивість
-- гаманця, а не «він у списку X». Один гаманець може бути і в KNOWN_CEX, і в
-- ENTITIES; токен — і в universe, і в переліку ETH-представників.
--
-- Тег — це рівно те, чим був масив у коді: іменована множина.

CREATE TABLE wallet_tags (
  chain    TEXT NOT NULL,
  address  TEXT NOT NULL,
  tag      TEXT NOT NULL,          -- 'known_cex', 'paribu', 'burn', 'expand_bootstrap'
  source   TEXT NOT NULL,          -- звідки переїхало, для аудиту
  notes    TEXT,
  PRIMARY KEY (chain, address, tag),
  FOREIGN KEY (chain, address) REFERENCES wallets(chain, address) ON DELETE CASCADE
) STRICT;
CREATE INDEX wallet_tags_tag ON wallet_tags(tag);

CREATE TABLE token_tags (
  chain    TEXT NOT NULL,
  contract TEXT NOT NULL,
  tag      TEXT NOT NULL,          -- 'eth_representative', 'stable', …
  source   TEXT NOT NULL,
  notes    TEXT,
  PRIMARY KEY (chain, contract, tag),
  FOREIGN KEY (chain, contract) REFERENCES tokens(chain, contract) ON DELETE CASCADE
) STRICT;
CREATE INDEX token_tags_tag ON token_tags(tag);
