// Регенерує golden-фікстуру парсера token-transfers (blockscout.rs parse_transfers_rich)
// З ПРОДАКШН-КОДУ. Чиста частина transfersRich() у server/blockscout.js не експортується окремо
// (вона всередині мережевого циклу), тому маппінг одного item продубльовано тут один-в-один —
// diff цих 15 рядків проти blockscout.js:235-254 читається очима.
// Запуск: node tools/gen_bs_fixtures.mjs → rust-server/tests/fixtures/bs-golden.json
import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const FIX = join(dirname(fileURLToPath(import.meta.url)), "..", "rust-server", "tests", "fixtures");
const d = JSON.parse(readFileSync(join(FIX, "bs-input.json"), "utf8"));

const out = [];
for (const it of d.items || []) {
  const dec = Number(it.total?.decimals || 18);
  const amount = Number(it.total?.value || 0) / 10 ** dec;
  const rate = it.token?.exchange_rate ? Number(it.token.exchange_rate) : 0;
  out.push({
    from: (it.from?.hash || "").toLowerCase(),
    to: (it.to?.hash || "").toLowerCase(),
    fromName: it.from?.name || it.from?.ens_domain_name || "",
    toName: it.to?.name || it.to?.ens_domain_name || "",
    fromContract: !!it.from?.is_contract,
    toContract: !!it.to?.is_contract,
    asset: it.token?.symbol || "?",
    token: (it.token?.address || it.token?.address_hash || "").toLowerCase(),
    amount,
    usd: rate ? amount * rate : 0,
    hash: (it.transaction_hash || it.tx_hash || "").toLowerCase(),
    ts: Date.parse(it.timestamp) || 0,
  });
}
writeFileSync(join(FIX, "bs-golden.json"), JSON.stringify(out, null, 2) + "\n");
console.log(`bs-golden.json оновлено: ${out.length} переказів, поля: ${Object.keys(out[0] || {}).join(", ")}`);
