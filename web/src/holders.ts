/* ---------- Топ-тримачі активу ---------- */

import type { EntityType } from "./types";
import { t } from "./i18n";
import { $, fmtUsd, fmtNum, fmtPrice, shortAddr } from "./util";
import { state } from "./state";
import { ENTITY_TYPE_CLS } from "./attribution";
import { priceOf, setPrice } from "./prices";
import { fetchCgPrices, getJSON } from "./api";
import { assetColor, ASSET_DIR } from "./assets";


/* ВИДАЛЕНО 24.09.2026: список із восьми зашитих «холдерів» (Binance, Wintermute, Jump…),
   яким тут же приписувались баланси `50M + rnd()*1.5B`. Це були не приблизні дані й не
   заглушка — це були числа, вигадані в браузері й показані поруч зі справжніми
   балансами відстежуваних гаманців, без жодної позначки.

   Тепер: якщо відомий контракт — беремо РЕАЛЬНИЙ топ холдерів із /api/concentration
   (частки від обігу, з мітками сутностей). Якщо контракт невідомий — показуємо лише
   те, що справді знаємо: наші відстежувані гаманці, які тримають цей актив, і чесно
   кажемо, що повного списку холдерів для цього активу в нас немає. */

let assetSym: string | null = null;
let assetCg: string | null = null;
let assetName: string | null = null;
let assetContract: string | null = null;

interface ConcHolder { rank: number; address: string; share: number; cum: number; cls?: string; label?: string }
interface ConcData { token: { name: string; symbol: string; holdersCount: number; contract: string }; holders: ConcHolder[] }

interface HolderRow { name: string; kind: "wallet" | "entity"; etype?: EntityType; amount: number; value: number; wid?: string; }

/** Гарантує наявність ціни активу: бере з кешу або підвантажує з CoinGecko за id. */
export async function ensurePrice(sym: string, cgId: string | null): Promise<number> {
  const cached = priceOf(sym);
  if (cached) return cached;
  if (!cgId) return 0;
  try {
    const d = await fetchCgPrices(cgId);
    const v = d[cgId]?.usd;
    if (typeof v === "number") { setPrice(sym, v); return v; }
  } catch { /* лишаємось без ціни */ }
  return priceOf(sym);
}

/** Реальні холдери з ланцюга — лише коли відомий контракт. */
function realHoldersHtml(d: ConcData): string {
  return d.holders.slice(0, 20).map((h) => {
    const label = h.label ? `<span class="px-1.5 py-0.5 rounded bg-panel2 border border-edge text-[10px]">${h.label}</span>` : "";
    return `
      <div data-wallet-addr="${h.address}" class="flex items-center gap-3 py-2 border-b border-edge/60 cursor-pointer hover:bg-panel2/50">
        <span class="w-6 text-center text-slate-500 text-xs">${h.rank}</span>
        <div class="flex-1 min-w-0">
          <div class="flex justify-between gap-2 text-sm"><span class="font-mono truncate">${shortAddr(h.address)} ${label}</span><span>${h.share.toFixed(2)}%</span></div>
          <div class="flex justify-between gap-2 text-xs text-slate-400"><span>${t("holders_cum")}: ${h.cum.toFixed(1)}%</span></div>
          <div class="mt-1 h-1 rounded bg-panel2"><div class="h-1 rounded bg-accent" style="width:${Math.min(100, h.share).toFixed(1)}%"></div></div>
        </div>
      </div>`;
  }).join("");
}

export async function openAssetHolders(sym: string, cgId: string | null = null, name: string | null = null, contract: string | null = null): Promise<void> {
  assetSym = sym; assetCg = cgId; assetName = name; assetContract = contract;
  const price = priceOf(sym) || (await ensurePrice(sym, cgId));
  if (assetSym !== sym) return; // користувач уже відкрив інше
  const priced = price > 0;
  const rows: HolderRow[] = [];
  state.wallets.forEach((w) => {
    const h = w.holdings.find((x) => x.asset === sym);
    if (h) rows.push({ name: w.label || shortAddr(w.address), kind: "wallet", amount: h.amount, value: h.amount * price, wid: w.id });
  });
  const weight = (r: HolderRow): number => (priced ? r.value : r.amount);
  rows.sort((a, b) => weight(b) - weight(a));
  const top = rows.slice(0, 12);
  const totalW = top.reduce((s, r) => s + weight(r), 0) || 1;
  const nameOf = name ?? ASSET_DIR.find((a) => a.sym === sym)?.name ?? sym;

  // Справжній топ холдерів — окремим блоком і лише за наявності контракту.
  let chainHtml = "";
  if (contract) {
    try {
      const d = await getJSON<ConcData>("/api/concentration", { contract });
      if (assetSym !== sym) return;
      chainHtml = `<div class="text-xs text-slate-400 mt-3">${t("holders_onchain")} (${d.token.holdersCount.toLocaleString()})</div><div>${realHoldersHtml(d)}</div>`;
    } catch {
      chainHtml = `<div class="text-[11px] text-amber-400/80 mt-3">${t("holders_chain_err")}</div>`;
    }
  } else {
    chainHtml = `<div class="text-[11px] text-amber-400/80 mt-3">${t("holders_nocontract")}</div>`;
  }

  const list = top.map((r, i) => {
    const pct = (weight(r) / totalW) * 100;
    const badge = r.kind === "entity"
      ? `<span class="px-1.5 py-0.5 rounded ${ENTITY_TYPE_CLS[r.etype as EntityType]} text-[10px]">${t(`entity_${r.etype}`)}</span>`
      : `<span class="px-1.5 py-0.5 rounded bg-accent/15 text-accent text-[10px]">${t("g_wallet_one")}</span>`;
    return `
      <div ${r.kind === "wallet" ? `data-wallet="${r.wid}"` : ""} class="flex items-center gap-3 py-2 border-b border-edge/60 ${r.kind === "wallet" ? "cursor-pointer hover:bg-panel2/50" : ""}">
        <span class="w-6 text-center text-slate-500 text-xs">${i + 1}</span>
        <div class="flex-1 min-w-0">
          <div class="flex justify-between gap-2 text-sm"><span class="font-medium truncate">${r.name} ${badge}</span><span>${priced ? fmtUsd(r.value) : "—"}</span></div>
          <div class="flex justify-between gap-2 text-xs text-slate-400"><span class="font-mono truncate">${fmtNum(r.amount, 2)} ${sym}</span><span>${pct.toFixed(1)}%</span></div>
          <div class="mt-1 h-1 rounded bg-panel2"><div class="h-1 rounded bg-accent" style="width:${pct.toFixed(1)}%"></div></div>
        </div>
      </div>`;
  }).join("");

  $("#asset-body").innerHTML = `
    <div class="flex items-center gap-2">
      <span class="w-8 h-8 rounded-full grid place-items-center text-[10px] font-bold text-black" style="background:${assetColor(sym)}">${sym.slice(0, 4)}</span>
      <div><div class="font-semibold">${nameOf} <span class="text-slate-400 text-sm">${sym}</span></div>
      <div class="text-xs text-slate-400">${priced ? t("pf_price") + " " + fmtPrice(price) : t("price_unknown")}</div></div>
    </div>
    ${top.length ? `<div class="text-xs text-slate-400">${t("holders_ours")} (${top.length})</div><div>${list}</div>` : ""}
    ${chainHtml}
    <div class="text-[11px] text-slate-500">${t("holders_note")}</div>`;

  const modal = $("#asset-modal");
  modal.classList.remove("hidden"); modal.classList.add("flex");
}

export function closeAssetHolders(): void {
  assetSym = null; assetCg = null; assetName = null;
  const modal = $("#asset-modal");
  modal.classList.add("hidden"); modal.classList.remove("flex");
}


/** Перемалювати панель тримачів під поточний актив (напр., після зміни мови/цін). */
export function refreshAssetHolders(): void {
  if (assetSym) void openAssetHolders(assetSym, assetCg, assetName, assetContract);
}
