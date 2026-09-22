// Міграція dataset/wallets.json на схему ознак v2 (див. server/features.js).
//  - spamRatio    → ВИДАЛЕНО: у всіх 109 рядках = 0 (мертва константа, std=0→1 у моделі)
//  - timespanDays → windowDays: воно міряло не вік гаманця, а час набору вікна переказів
//  - txPerDay     → перераховано з avgGapMin (1440/avgGapMin): старе txCount/max(days,0.01)
//                   впиралось у штучну стелю 5000/добу — артефакт запобіжника від ділення на нуль
// Запуск: node tools/migrate_wallets_dataset.mjs
import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const FILE = join(dirname(fileURLToPath(import.meta.url)), "..", "dataset", "wallets.json");
const rows = JSON.parse(readFileSync(FILE, "utf8"));

let capped = 0, migrated = 0;
const out = rows.map((r) => {
  if (r.windowDays !== undefined) return r; // вже мігровано
  migrated++;
  if (r.txPerDay >= 5000) capped++;
  const { spamRatio, timespanDays, txPerDay, ...rest } = r;
  return {
    ...rest,
    windowTx: 50, // фактичний розмір сторінки, з якої зібрано ці рядки
    truncated: (r.txCount || 0) >= 50,
    windowDays: timespanDays,
    txPerDay: r.avgGapMin > 0 ? +(1440 / r.avgGapMin).toFixed(2) : 0,
  };
});

writeFileSync(FILE, JSON.stringify(out, null, 2));
const tp = out.map((r) => r.txPerDay).sort((a, b) => a - b);
console.log(`мігровано рядків: ${migrated}/${rows.length}`);
console.log(`  txPerDay: раніше ${capped} рядків стояли рівно на штучній стелі 5000/добу`);
console.log(`  тепер: min ${tp[0]} · медіана ${tp[tp.length >> 1]} · max ${tp[tp.length - 1]}`);
console.log(`  spamRatio видалено (був 0 у всіх рядках), timespanDays → windowDays`);
