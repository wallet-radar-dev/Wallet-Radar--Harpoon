/* ---------- Entity-профілі (сторінка сутності) ---------- */

import type { EntityType } from "./types";
import { t } from "./i18n";
import { $, fmtUsd, shortAddr, randomHex, seededRand } from "./util";
import { priceOf } from "./prices";
import { state } from "./state";
import { networkById, EVM_CHAINS } from "./networks";
import { arkhamLink, ENTITY_TYPE_CLS, ENTITY_HEX } from "./attribution";
import { assetColor } from "./assets";
import { clarityBadge } from "./clarity";


let entityProfile: { name: string; etype: EntityType } | null = null;

const arkhamEntityLink = (name: string): string =>
  `https://intel.arkm.com/explorer/entity/${name.toLowerCase().replace(/\s+/g, "-")}`;

function buildEntityHtml(name: string, etype: EntityType): string {
  const rnd = seededRand("entity:" + name);
  const totalTarget = 5e8 + rnd() * 5e9;
  const assets = ["BTC", "ETH", "USDT", "USDC", "SOL", "BNB"];
  const chosen = [...assets].sort(() => rnd() - 0.5).slice(0, 3 + Math.floor(rnd() * 3));
  const weights = chosen.map(() => 0.1 + rnd());
  const sw = weights.reduce((a, b) => a + b, 0);
  const holdings = chosen.map((a, i) => { const usd = totalTarget * (weights[i] / sw); return { asset: a, amount: usd / (priceOf(a) || 1), value: usd }; })
    .sort((a, b) => b.value - a.value);
  const total = holdings.reduce((s, h) => s + h.value, 0) || 1;

  let acc = 0;
  const stops = holdings.map((h) => { const s = (acc / total) * 100; acc += h.value; return `${assetColor(h.asset)} ${s.toFixed(2)}% ${((acc / total) * 100).toFixed(2)}%`; }).join(", ");

  const chains = EVM_CHAINS.concat(["solana", "bitcoin"]);
  const subWallets = Array.from({ length: 3 + Math.floor(rnd() * 5) }, () => {
    const isEvm = rnd() > 0.3;
    const addr = isEvm ? "0x" + randomHex(40) : randomHex(44);
    return { addr, net: isEvm ? chains[Math.floor(rnd() * EVM_CHAINS.length)] : (rnd() > 0.5 ? "solana" : "bitcoin") };
  });

  const interacts = state.wallets.filter((w) => state.events.some((e) => e.walletId === w.id && e.counterpartyEntity.type !== "unknown" && e.counterpartyEntity.name === name));

  const assetList = holdings.map((h) => {
    const pct = (h.value / total) * 100;
    return `<div class="flex items-center gap-3 py-1.5 border-b border-edge/60">
      <span class="w-6 h-6 rounded-full grid place-items-center text-[9px] font-bold text-black shrink-0" style="background:${assetColor(h.asset)}">${h.asset.slice(0, 4)}</span>
      <div class="flex-1 min-w-0"><div class="flex justify-between text-sm"><span>${h.asset}${clarityBadge(h.asset)}</span><span>${fmtUsd(h.value)}</span></div>
      <div class="h-1 mt-1 rounded bg-panel2"><div class="h-1 rounded" style="width:${pct.toFixed(1)}%;background:${assetColor(h.asset)}"></div></div></div>
      <span class="text-xs text-slate-400 shrink-0">${pct.toFixed(1)}%</span></div>`;
  }).join("");

  const subList = subWallets.map((s) => {
    const net = networkById(s.net);
    return `<div class="flex items-center justify-between gap-2 py-1.5 text-xs border-b border-edge/60">
      <span class="font-mono truncate">${shortAddr(s.addr)} <span class="${net.color}">${net.name}</span></span>
      <a href="${arkhamLink(s.addr)}" target="_blank" rel="noopener" class="text-purple-400 hover:underline shrink-0">🔍 Arkham →</a></div>`;
  }).join("");

  const interactsHtml = interacts.length
    ? interacts.map((w) => `<button data-wallet="${w.id}" class="w-full flex items-center justify-between gap-2 text-xs bg-panel2 hover:border-accent border border-edge rounded-lg px-2 py-1.5 transition"><span class="truncate">${w.label || shortAddr(w.address)}</span><span class="text-slate-500">→</span></button>`).join("")
    : `<div class="text-xs text-slate-500">—</div>`;

  return `
    <div class="flex items-center gap-3">
      <span class="w-10 h-10 rounded-xl grid place-items-center text-black font-bold" style="background:${ENTITY_HEX[etype]}">${name.slice(0, 2)}</span>
      <div>
        <div class="text-lg font-semibold">${name}</div>
        <div class="flex items-center gap-2"><span class="px-2 py-0.5 rounded-full ${ENTITY_TYPE_CLS[etype]} text-[10px]">${t(`entity_${etype}`)}</span>
        <a href="${arkhamEntityLink(name)}" target="_blank" rel="noopener" class="text-xs text-purple-400 hover:underline">🔍 Arkham Intel →</a></div>
      </div>
    </div>

    <div class="flex items-center gap-5">
      <div class="relative w-32 h-32 rounded-full shrink-0" style="background: conic-gradient(${stops})">
        <div class="absolute inset-[24%] rounded-full bg-panel grid place-items-center text-center">
          <div><div class="text-[10px] text-slate-400">${t("ent_balance")}</div><div class="text-xs font-semibold">${fmtUsd(total)}</div></div>
        </div>
      </div>
      <div class="flex-1 min-w-0">${assetList}</div>
    </div>

    <div><div class="text-xs text-slate-400 mb-1">${t("ent_addresses")} (${subWallets.length})</div>${subList}</div>
    <div><div class="text-xs text-slate-400 mb-1">${t("ent_interacts")} (${interacts.length})</div><div class="space-y-1">${interactsHtml}</div></div>
    <div class="text-[11px] text-slate-500">${t("holders_note")}</div>`;
}

export function openEntityProfile(name: string, etype: EntityType): void {
  entityProfile = { name, etype };
  $("#entity-body").innerHTML = buildEntityHtml(name, etype);
  const m = $("#entity-modal");
  m.classList.remove("hidden"); m.classList.add("flex");
}

export function closeEntityProfile(): void {
  entityProfile = null;
  const m = $("#entity-modal");
  m.classList.add("hidden"); m.classList.remove("flex");
}


/** Перемалювати профіль сутності під поточну (напр., після зміни мови). */
export function refreshEntity(): void {
  if (entityProfile) openEntityProfile(entityProfile.name, entityProfile.etype);
}
