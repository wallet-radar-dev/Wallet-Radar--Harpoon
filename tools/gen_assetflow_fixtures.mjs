// Регенерує golden-фікстуру assetflow для Rust-порту З ПРОДАКШН-КОДУ (чисте ядро reduceAssetFlow).
// Вхід (assetflow-input.json) лишається як є — переобчислюється лише еталонний вихід.
// Запуск: node tools/gen_assetflow_fixtures.mjs
import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { reduceAssetFlow } from "../server/assetflow.js";

const FIX = join(dirname(fileURLToPath(import.meta.url)), "..", "rust-server", "tests", "fixtures");
const inputs = JSON.parse(readFileSync(join(FIX, "assetflow-input.json"), "utf8"));
const golden = inputs.map((c) =>
  reduceAssetFlow(c.contract, c.info, c.holders, c.per, c.series, c.mc, c.social, c.now),
);
writeFileSync(join(FIX, "assetflow-golden.json"), JSON.stringify(golden, null, 2) + "\n");
console.log(`assetflow-golden.json оновлено: ${golden.length} кейсів`);
for (const [i, g] of golden.entries()) {
  console.log(`  кейс ${i}: avgEntry=${g.avgEntry?.price ?? "null"} phase=${g.whalePnl.phase} basis=${g.whalePnl.basis}`);
}
