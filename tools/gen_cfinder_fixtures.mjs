// Регенерує golden-фікстури для Rust-порту детектора диригентів (conductor_finder.rs)
// З САМОГО ПРОДАКШН-КОДУ: тут відтворено collect()+кандидатний конвеєр із
// server/conductor-finder.js один-в-один (сам модуль не експортує їх окремо, бо читає
// файли й ходить у мережу — тому чиста частина продубльована тут, і будь-яка розбіжність
// із JS ловиться очима на diff цих 20 рядків, а не на 500 рядках Rust).
// Запуск: node tools/gen_cfinder_fixtures.mjs → rust-server/tests/fixtures/cfinder-{input,golden}.json
import { writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { CEX } from "../server/cex.js";
import { entityOf } from "../server/entities.js";
import { isInfraAddr } from "../server/labels.js";

const FIX = join(dirname(fileURLToPath(import.meta.url)), "..", "rust-server", "tests", "fixtures");
const MIN_REUSED_HOLDERS = 2;

// ── conductor-finder.js: collect() + buildConductors() до верифікації ──
function candidates(assets) {
  const agg = new Map();
  for (const a of assets) {
    for (const group of a.groups || []) {
      for (const f of group.commonFunders || []) {
        const fa = (f.funder || "").toLowerCase();
        if (!/^0x[0-9a-f]{40}$/.test(fa) || CEX.has(fa) || entityOf(fa) || isInfraAddr(fa)) continue;
        const e = agg.get(fa) || { byAsset: new Map(), maxBacker: 0, totalBacker: 0, n: 0 };
        const s = e.byAsset.get(a.symbol) || new Set();
        (group.members || []).forEach((m) => s.add(m.addr));
        e.byAsset.set(a.symbol, s);
        e.maxBacker = Math.max(e.maxBacker, f.avgUsd || 0); e.totalBacker += f.avgUsd || 0; e.n++;
        agg.set(fa, e);
      }
    }
  }
  const scored = [...agg.entries()].map(([funder, e]) => {
    const assetsL = [...e.byAsset.keys()];
    const seen = new Map();
    for (const s of e.byAsset.values()) for (const h of s) seen.set(h, (seen.get(h) || 0) + 1);
    const reused = [...seen.entries()].filter(([, n]) => n >= 2).map(([h]) => h);
    return {
      funder, assetCount: assetsL.length, assets: assetsL,
      holderCount: seen.size, holders: [...seen.keys()].slice(0, 12),
      reusedHolders: reused.length, reusedSample: reused.slice(0, 8),
      maxBackerUsd: Math.round(e.maxBacker), avgBackerUsd: Math.round(e.totalBacker / (e.n || 1)),
    };
  });
  const list = scored
    .filter((c) => c.assetCount >= 2)
    .filter((c) => c.reusedHolders >= MIN_REUSED_HOLDERS)
    .sort((a, b) => (b.reusedHolders - a.reusedHolders) || (b.assetCount - a.assetCount) || (b.maxBackerUsd - a.maxBackerUsd))
    .slice(0, 60);
  const dropped = scored.filter((c) => c.assetCount >= 2).length - list.length;
  return { candidates: list, dropped };
}

const grp = (funder, avgUsd, members) => ({ commonFunders: [{ funder, avgUsd }], members: members.map((addr) => ({ addr })) });
const A = "0xf30ba13e00000000000000000000000000000001"; // диригент: та сама команда в кількох активах
const B = "0xf30ba13e00000000000000000000000000000002"; // диспенсер: щоразу нові адреси
const C = "0xf30ba13e00000000000000000000000000000003"; // один актив → взагалі не кандидат
const BINANCE = "0x28c6c06298d514db089934071355e5743bf21d60"; // CEX → відсіюється лейблами

const cases = [
  // 0 — порожній вхід
  [],
  // 1 — диригент проти диспенсера проти одноактивного, плюс біржа
  [
    { symbol: "UNI", groups: [grp(A, 2_500_000, ["0xaa", "0xbb", "0xcc"]), grp(B, 900_000, ["0xb1", "0xb2"]), grp(C, 100, ["0xc1"]), grp(BINANCE, 9e9, ["0xe1", "0xe2"])] },
    { symbol: "LINK", groups: [grp(A, 1_000_000, ["0xaa", "0xbb"]), grp(B, 400_000, ["0xb3", "0xb4"])] },
    { symbol: "AAVE", groups: [grp(A, 3_000_000, ["0xbb", "0xdd"])] },
  ],
  // 2 — рівно на порозі: 1 повторний гаманець (<MIN_REUSED_HOLDERS) → диспенсер
  [
    { symbol: "UNI", groups: [grp(A, 500, ["0xaa", "0xzz"])] },
    { symbol: "LINK", groups: [grp(A, 700, ["0xaa", "0xyy"])] },
  ],
  // 3 — той самий фандер у двох групах одного активу: активів усе одно 1 → не кандидат
  [{ symbol: "UNI", groups: [grp(A, 10, ["0xaa"]), grp(A, 20, ["0xbb"])] }],
];

writeFileSync(join(FIX, "cfinder-input.json"), JSON.stringify(cases, null, 2));
writeFileSync(join(FIX, "cfinder-golden.json"), JSON.stringify(cases.map(candidates), null, 2));
console.log(`cfinder: ${cases.length} кейсів → фікстури оновлено`);
for (const [i, r] of cases.map(candidates).entries()) console.log(`  кейс ${i}: кандидатів ${r.candidates.length}, відкинуто як диспенсерів ${r.dropped}`);
