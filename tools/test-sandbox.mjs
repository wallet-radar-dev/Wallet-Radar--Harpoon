// Пісочниця для тестів: тимчасовий dataset/, куди скопійовано лише те, що тести ЧИТАЮТЬ.
//
// Навіщо. Модулі server/ пишуть у dataset/ (27 із 34), і `node --test` через це затирав БОЙОВІ дані:
// buildWhaleClusters, викликана тестом із порожнім входом, клала count:0 поверх
// dataset/whale-clusters.json; так само страждали strategy-phases.json і server/data.json.
// Тепер DATASET_DIR вказує сюди — записи лягають на копію, продакшн недосяжний.
//
// Копіюємо, а не симлінкуємо: writeFileSync крізь симлінк переписав би ОРИГІНАЛ, тобто «захист»
// зробив би рівно те, від чого захищає.
//
// Друкує шлях у stdout — package.json підставляє його в DATASET_DIR.
import { mkdtempSync, copyFileSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");

// Вхідні дані, без яких тести не мають сенсу: мітки (біржі, інфраструктура, burn-адреси) і
// натренована модель (model.test.ts класифікує нею реальні вектори ознак).
// Решта dataset/ тестам не потрібна — і копіювати 40 МБ графа на кожен прогін теж ні до чого.
const READ_ONLY_INPUTS = ["labels-cache.json", "model.json"];

const sandbox = mkdtempSync(join(tmpdir(), "wallet-radar-test-"));
for (const f of READ_ONLY_INPUTS) {
  const src = join(ROOT, "dataset", f);
  if (existsSync(src)) copyFileSync(src, join(sandbox, f));
}
process.stdout.write(sandbox);
