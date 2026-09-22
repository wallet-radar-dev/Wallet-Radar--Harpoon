// Регенерує golden-фікстуру whale-clusters для Rust-порту З ПРОДАКШН-КОДУ (buildWhaleClusters).
// Вхід (wc-input.json) лишається як є; переобчислюється лише еталонний вихід.
// `generated` (Date.now) із фікстури прибирається — Rust його не рахує, це робить викликач.
// Запуск: node tools/gen_wc_fixtures.mjs
// ГЕРМЕТИЧНІСТЬ: Rust-тест вантажить ПОРОЖНЮ базу лейблів, тож і еталон має рахуватись без неї —
// інакше golden залежав би від живого labels-cache.json (оновлюється кожні 12 год) і паритет
// «ламався» б сам, без жодної зміни коду.
process.env.WR_LABELS_OFF = "1";

import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
const { buildWhaleClusters } = await import("../server/whale-clusters.js"); // після встановлення env

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const FIX = join(ROOT, "rust-server", "tests", "fixtures");
const LIVE = join(ROOT, "dataset", "whale-clusters.json");

// buildWhaleClusters пише бойовий whale-clusters.json як побічний ефект — зберігаємо й повертаємо.
const backup = readFileSync(LIVE, "utf8");
try {
  const input = JSON.parse(readFileSync(join(FIX, "wc-input.json"), "utf8"));
  const out = buildWhaleClusters(input);
  delete out.generated; // недетермінований timestamp — не частина контракту
  writeFileSync(join(FIX, "wc-golden.json"), JSON.stringify(out, null, 2) + "\n");
  console.log(`wc-golden.json оновлено: ${out.count} кластерів`);
  console.log(`  розміри: ${out.clusters.map((c) => c.members.length).join(", ") || "—"}`);
  console.log(`  стеля: ${out.diagnostics.maxClusterSize} · нерозділені: ${out.diagnostics.unresolvedBlobs.length || "немає"}`);
} finally {
  writeFileSync(LIVE, backup); // відновлюємо бойовий датасет
}
