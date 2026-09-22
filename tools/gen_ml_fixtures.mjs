// Регенерує golden-фікстури для Rust-порту ML (smart_money.rs) З САМОГО ПРОДАКШН-КОДУ.
// Джерело правди — чисті функції server/ml-smart-money.js: smFeatures() і smStatusBuild().
// Запуск: node tools/gen_ml_fixtures.mjs   → rust-server/tests/fixtures/{smpredict,mlstatus}-{input,golden}.json
import { writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { smFeatures, smStatusBuild, FEATS } from "../server/ml-smart-money.js";

const FIX = join(dirname(fileURLToPath(import.meta.url)), "..", "rust-server", "tests", "fixtures");
const prob = (m, x) => {
  let z = m.b;
  for (let j = 0; j < m.w.length; j++) z += m.w[j] * ((x[j] - m.mean[j]) / (m.std[j] || 1));
  return 1 / (1 + Math.exp(-z));
};
const round1 = (v) => +v.toFixed(1);

// Модель на 8 ознак: ваги довільні, але детерміновані — тест перевіряє арифметику, не якість.
const M8 = {
  w: [0.04, -0.03, 0.025, 0.01, -0.005, 0.06, 0.015, 0.035],
  b: -0.2,
  mean: [5, 6, 3, 2, 4.6, 0.4, 4, 7],
  std: [2, 2.5, 1.5, 1, 1.8, 0.9, 3, 2.2],
};
// Крайні випадки: нульовий std (константна ознака → || 1) і порожній flow (Null → усі нулі).
const M8_ZEROSTD = { ...M8, std: [2, 0, 1.5, 1, 1.8, 0.9, 0, 2.2] };

const smInputs = [
  { flow: { inUsd: 1065, outUsd: 891529, txTotal: 569, chains: 6, timespanDays: 2107.28 }, tokens: [{ symbol: "USDC", valueUsd: 1000.4 }, { symbol: "PEPE", valueUsd: 250.7 }, { symbol: "USDC", valueUsd: 500 }], model: M8 },
  { flow: null, tokens: [], model: M8 },
  { flow: { inUsd: 0, outUsd: 0, txTotal: 0, chains: 0, timespanDays: 0 }, tokens: [], model: M8_ZEROSTD },
  { flow: { inUsd: 42, outUsd: 0, txTotal: 3, chains: 1, timespanDays: 0.5 }, tokens: [{ symbol: "WETH", valueUsd: 0 }], model: M8 },
  // Гаманець-«вихід без входу»: out/in вибухає (раніше — 21 сигма сирого викиду, тепер під log1p).
  { flow: { inUsd: 0.0001, outUsd: 4.1e9, txTotal: 12000, chains: 8, timespanDays: 3650 }, tokens: [{ symbol: "USDT", valueUsd: 9.9e8 }], model: M8 },
];
const smGolden = smInputs.map((c) => {
  const x = smFeatures(c.flow, c.tokens);
  return { x, prob1: round1(prob(c.model, x) * 100) };
});

// mlStatus: активна модель, неактивна (не пройшла gate), і відсутня.
const active = { w: M8.w, b: M8.b, mean: M8.mean, std: M8.std, feats: FEATS, schema: 2, active: true, acc: 71, cvAcc: 67, cvAuc: 0.725, nullAcc: 51, nullAuc: 0.522, samples: 890, posRate: 12, updatedAt: 1783763496680 };
const gated = { ...active, active: false, cvAcc: 53, cvAuc: 0.54, nullAcc: 51, nullAuc: 0.522 };
const mlInputs = [
  { model: active, samples: 890 },
  { model: gated, samples: 890 },
  { model: null, samples: 0 },
];
const mlGolden = mlInputs.map((c) => smStatusBuild(c.model, c.samples));

// ── ml-input/ml-golden: тренування + УСЯ метрична обв'язка на РЕАЛЬНОМУ датасеті ──
// Rust має відтворити не лише ваги, а й нульову лінію (перемішування на LCG) та рішення gate.
// Підмножина, а не весь датасет: cargo test має лишатись швидким (нульова лінія = 5 повних CV).
import { readFileSync } from "node:fs";
import { smTrain, smEvaluate } from "../server/ml-smart-money.js";

const full = JSON.parse(readFileSync(join(FIX, "..", "..", "..", "dataset", "sm-train.json"), "utf8"));
const pos = full.filter((d) => d.y === 1).slice(0, 30);
const neg = full.filter((d) => d.y === 0).slice(0, 170);
const mlData = []; // чергуємо, щоб порядок був детермінованим і не сортованим за міткою
for (let i = 0; i < Math.max(pos.length, neg.length); i++) {
  if (i < pos.length) mlData.push({ x: pos[i].x, y: pos[i].y });
  if (i < neg.length) mlData.push({ x: neg[i].x, y: neg[i].y });
}
const mdl = smTrain(mlData.map((d) => d.x), mlData.map((d) => d.y));
const ev = smEvaluate(mlData, mdl);
const mlGoldenTrain = {
  mean: mdl.mean, std: mdl.std, w: mdl.w, b: mdl.b,
  balAcc: ev.acc, cvAcc: ev.cvAcc, cvAuc: ev.cvAuc, nullAcc: ev.nullAcc, nullAuc: ev.nullAuc, active: ev.active,
  importance: FEATS.map((f, j) => ({ f, w: +mdl.w[j].toFixed(2) })).sort((a, b) => Math.abs(b.w) - Math.abs(a.w)).slice(0, 5),
};

const w = (name, v) => writeFileSync(join(FIX, name), JSON.stringify(v, null, 2) + "\n");
w("smpredict-input.json", smInputs);
w("smpredict-golden.json", smGolden);
w("mlstatus-input.json", mlInputs);
w("mlstatus-golden.json", mlGolden);
w("ml-input.json", mlData);
w("ml-golden.json", mlGoldenTrain);
console.log(`фікстури оновлено: smpredict ${smInputs.length}, mlstatus ${mlInputs.length}, ml ${mlData.length} семплів (${FEATS.length} ознак)`);
console.log(`  ml-golden: CV ${ev.cvAcc}% / AUC ${ev.cvAuc} · нульова лінія ${ev.nullAcc}% / AUC ${ev.nullAuc} · gate: ${ev.active ? "активна" : "НЕАКТИВНА"}`);
