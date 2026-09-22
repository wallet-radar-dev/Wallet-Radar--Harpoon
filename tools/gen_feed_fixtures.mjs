// Полагоджує counterpartyEntity у golden-фікстурах стрічки (toevent/moralis).
//
// ЧОМУ ЦЕЙ СКРИПТ ІСНУЄ. Ці два goldens — єдині у проєкті, які НЕ знімались із Node: їх зняли з
// виходу самого Rust, тож вони закріпили його ж помилку (counterpartyEntity: null) і тест
// `*_matches_node` роками «підтверджував» те, чого Node ніколи не робив. Тут поле рахується
// незалежно від Rust — із Node-івських джерел: ENTITIES (server/entities.js) + KNOWN_CEX
// (server/feed.js). Решта полів у фікстурах не чіпається.
//
// Контракт (feed.js:163): для стрічки entityOf НІКОЛИ не повертає null.
//   відома адреса з ENTITIES → {name: <ім'я>, type: <тип>}   (збагачення з порту entities)
//   адреса з KNOWN_CEX       → {name: "CEX",  type: "cex"}
//   будь-яка інша            → {name: "",     type: "unknown"}
//
// Запуск: node tools/gen_feed_fixtures.mjs

import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { ENTITIES } from "../server/entities.js";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const FIX = join(root, "rust-server", "tests", "fixtures");

// KNOWN_CEX читаємо з живого коду feed.js, а не копіюємо: копія розійшлася б із джерелом мовчки.
const feedSrc = readFileSync(join(root, "server", "feed.js"), "utf8");
const cexBlock = feedSrc.match(/const KNOWN_CEX = new Set\(\[([\s\S]*?)\]\)/);
if (!cexBlock) throw new Error("KNOWN_CEX не знайдено у server/feed.js — фікстури не чіпаю");
const KNOWN_CEX = new Set((cexBlock[1].match(/0x[0-9a-fA-F]{40}/g) || []).map((a) => a.toLowerCase()));

function entityOf(addr) {
  const a = String(addr || "").toLowerCase();
  const known = ENTITIES[a];
  if (known) return { name: known.name, type: known.type };
  if (KNOWN_CEX.has(a)) return { name: "CEX", type: "cex" };
  return { name: "", type: "unknown" };
}

let touched = 0;
const fixEvent = (ev) => {
  if (!ev || typeof ev !== "object" || !("counterpartyEntity" in ev)) return ev;
  const next = entityOf(ev.counterparty);
  if (JSON.stringify(next) !== JSON.stringify(ev.counterpartyEntity)) touched++;
  ev.counterpartyEntity = next;
  return ev;
};

// toevent-golden: {token: [...], native: [...]}
const toev = JSON.parse(readFileSync(join(FIX, "toevent-golden.json"), "utf8"));
for (const key of ["token", "native"]) (toev[key] || []).forEach(fixEvent);
writeFileSync(join(FIX, "toevent-golden.json"), JSON.stringify(toev, null, 2) + "\n");

// moralis-golden: [[event, ...], ...]
const mor = JSON.parse(readFileSync(join(FIX, "moralis-golden.json"), "utf8"));
for (const caseEvents of mor) (Array.isArray(caseEvents) ? caseEvents : []).forEach(fixEvent);
writeFileSync(join(FIX, "moralis-golden.json"), JSON.stringify(mor, null, 2) + "\n");

console.log(`counterpartyEntity перераховано: ${touched} подій оновлено`);
