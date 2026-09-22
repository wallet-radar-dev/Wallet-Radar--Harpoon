// Регенерує golden-фікстуру класифікатора гаманців (rust-server/src/model.rs) З ПРОДАКШН-КОДУ.
// Rust-тест вантажить ЖИВИЙ dataset/model.json, тож після кожного перенавчання (server/train-model.js)
// еталон треба перерахувати — інакше паритет падає не через код, а через нову модель.
// Запуск: node tools/gen_model_fixtures.mjs
import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { classify, ready } from "../server/model.js";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const FIX = join(ROOT, "rust-server", "tests", "fixtures");
if (!ready()) throw new Error("dataset/model.json не завантажено");

const model = JSON.parse(readFileSync(join(ROOT, "dataset", "model.json"), "utf8"));
const F = model.features;
const vec = (vals) => Object.fromEntries(F.map((f, i) => [f, vals[i]]));

// Вектори ознак у НОВІЙ схемі (spamRatio прибрано, timespanDays → windowDays) + крайні випадки:
// порожній об'єкт, від'ємні значення, рядки замість чисел — усе, що модель має пережити без NaN.
const inputs = [
  vec(F.map(() => 0)),
  {}, // жодної ознаки — усе має впасти в 0, а не в NaN
  vec(F.map((_, i) => (i + 1) * 3)),
  vec(F.map((_, i) => (i % 2 ? -5 : 1000))), // від'ємні значення під log1p
  { ...vec(F.map(() => 1)), inCount: "42", nativeEth: "0.5" }, // рядки замість чисел
  vec([48, 2, 24, 3, 2, 16, 0.001, 0.02, 1.2, 0.9, 0.06, 0.98, 0.04, 1200].slice(0, F.length)), // схоже на біржу
  vec([12, 14, 0.85, 22, 19, 1.2, 340.5, 1800, 2600, 0.35, 0.12, 0.73, 0.55].slice(0, F.length)), // схоже на людину
];
const golden = inputs.map((f) => classify(f));

writeFileSync(join(FIX, "model-input.json"), JSON.stringify(inputs, null, 1) + "\n");
writeFileSync(join(FIX, "model-golden.json"), JSON.stringify(golden, null, 1) + "\n");
console.log(`model-фікстури оновлено: ${inputs.length} кейсів · тип моделі: ${model.type}`);
console.log(`  ознаки (${F.length}): ${F.join(", ")}`);
for (const [i, g] of golden.entries()) console.log(`  кейс ${i}: ${g.label} (${g.confidence}%)`);
