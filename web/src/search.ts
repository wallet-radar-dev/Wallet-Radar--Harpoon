/* ---------- Пошук активів (локальні підказки + CoinGecko) ---------- */

import { t } from "./i18n";
import { $, shortAddr, fmtUsd } from "./util";
import { state } from "./state";
import { escAttr } from "./ui";
import { getJSON } from "./api";
import { cgIdOf } from "./prices";
import { chainsForAddress } from "./networks";
import { assetColor, ASSET_DIR } from "./assets";
import { searchUniverse, sectorEmoji, type UAsset } from "./universe";
import { fetchCheck, isEvmAddress, renderCheckStrip } from "./check";


let searchSeq = 0;
let searchTimer = 0;


interface CoinHit { id: string; sym: string; name: string; rank: number | null; thumb: string | null; }

async function fetchCoinSearch(query: string): Promise<CoinHit[]> {
  try {
    const d = await getJSON<{ coins?: Array<{ id: string; symbol?: string; name: string; market_cap_rank?: number; thumb?: string }> }>("/api/cg-search", { query });
    return (d.coins ?? []).slice(0, 12).map((c): CoinHit => ({
      id: c.id, sym: String(c.symbol || "").toUpperCase(), name: c.name, rank: c.market_cap_rank ?? null, thumb: c.thumb ?? null,
    }));
  } catch { return []; }
}

function assetSearchBtn(sym: string, name: string, cg: string | null, rank: number | null, thumb: string | null): string {
  const icon = thumb
    ? `<img src="${thumb}" alt="" class="w-6 h-6 rounded-full" />`
    : `<span class="w-6 h-6 rounded-full grid place-items-center text-[9px] font-bold text-black" style="background:${assetColor(sym)}">${sym.slice(0, 4)}</span>`;
  const right = rank ? `#${rank}` : `${t("holders_title")} →`;
  return `<button data-s-type="asset" data-s-val="${escAttr(sym)}" ${cg ? `data-s-cg="${escAttr(cg)}"` : ""} data-s-name="${escAttr(name)}" class="w-full text-left px-4 py-2 hover:bg-panel2 flex items-center gap-3">
    ${icon}<span class="text-sm truncate">${name} <span class="text-slate-500">${sym}</span></span>
    <span class="ml-auto text-[11px] text-slate-500 shrink-0">${right}</span></button>`;
}

function uniSearchBtn(a: UAsset): string {
  const icon = `<span class="w-6 h-6 rounded-full grid place-items-center text-[9px] font-bold text-black" style="background:${assetColor(a.symbol)}">${a.symbol.slice(0, 4)}</span>`;
  const right = a.rank ? `#${a.rank}` : `${t("holders_title")} \u2192`;
  const sec = a.sector !== "other" ? `<span class="text-[10px] text-slate-500">${sectorEmoji(a.sector)} ${a.sector}</span>` : "";
  return `<button data-s-type="asset" data-s-val="${escAttr(a.symbol)}" data-s-contract="${escAttr(a.contract)}" data-s-name="${escAttr(a.name)}" class="w-full text-left px-4 py-2 hover:bg-panel2 flex items-center gap-3">
    ${icon}<span class="text-sm truncate">${a.name} <span class="text-slate-500">${a.symbol}</span> ${sec}</span>
    <span class="ml-auto text-[11px] text-slate-500 shrink-0">${right}</span></button>`;
}

function buildSearchHtml(raw: string, coins: CoinHit[]): string {
  const q = raw.toLowerCase();
  const uni = searchUniverse(raw, 15);
  const uniSyms = new Set(uni.map((a) => a.symbol));
  const localAssets = ASSET_DIR.filter((a) => (a.sym.toLowerCase().includes(q) || a.name.toLowerCase().includes(q)) && !uniSyms.has(a.sym));
  const localSyms = new Set<string>([...uniSyms, ...localAssets.map((a) => a.sym)]);
  const remote = coins.filter((c) => c.sym && !localSyms.has(c.sym)).slice(0, 10);
  const wallets = state.wallets.filter((w) => w.address.toLowerCase().includes(q) || (w.label || "").toLowerCase().includes(q)).slice(0, 5);
  const looksAddr = /^0x[0-9a-f]{4,}$/i.test(raw) || (raw.length >= 26 && /^[1-9a-hj-np-zA-HJ-NP-Z]+$/.test(raw));
  const trackedExact = state.wallets.some((w) => w.address.toLowerCase() === q);
  const sec = (label: string): string => `<div class="px-4 pt-2 pb-1 text-[11px] uppercase tracking-wide text-slate-500">${label}</div>`;

  let html = "";
  const assetItems = [
    ...uni.map((a) => uniSearchBtn(a)),
    ...localAssets.map((a) => assetSearchBtn(a.sym, a.name, cgIdOf(a.sym), null, null)),
    ...remote.map((c) => assetSearchBtn(c.sym, c.name, c.id, c.rank, c.thumb)),
  ];
  if (assetItems.length) html += sec(t("search_assets")) + assetItems.join("");
  if (wallets.length) {
    html += sec(t("search_wallets"));
    html += wallets.map((w) => `
      <button data-s-type="wallet" data-s-val="${w.id}" class="w-full text-left px-4 py-2 hover:bg-panel2 flex items-center gap-3">
        <span class="text-accent">●</span>
        <span class="min-w-0"><span class="text-sm">${w.label || shortAddr(w.address)}</span>
        <span class="block font-mono text-[11px] text-slate-500 truncate">${shortAddr(w.address)}</span></span>
        <span class="ml-auto text-xs text-slate-400">${fmtUsd(w.valueUsd)}</span>
      </button>`).join("");
  }
  if (looksAddr && !trackedExact) {
    const nets = chainsForAddress(raw).length;
    html += `<button data-s-type="address" data-s-val="${escAttr(raw)}" class="w-full text-left px-4 py-2.5 hover:bg-panel2 flex items-center gap-3 border-t border-edge">
      <span>🔎</span><span class="text-sm">${t("search_open_address")}: <span class="font-mono">${shortAddr(raw)}</span> <span class="text-slate-500">· ${nets} ${t("networks_word")}</span></span></button>`;
  }
  return html;
}

export function onSearchInput(val: string): void {
  const box = $("#search-results");
  const raw = val.trim();
  if (!raw) { box.classList.add("hidden"); box.innerHTML = ""; return; }
  box.innerHTML = buildSearchHtml(raw, []) + `<div class="px-4 py-2 text-[11px] text-slate-500">${t("search_loading")}</div>`;
  box.classList.remove("hidden");
  const seq = ++searchSeq;
  window.clearTimeout(searchTimer);
  searchTimer = window.setTimeout(async () => {
    // Перевірка адреси йде ПАРАЛЕЛЬНО з пошуком монет і нічого не блокує: вона
    // рахується з локальних наборів і повертається за мілісекунди, тоді як
    // CoinGecko може думати секунду. Якщо вона впаде — пошук працює як раніше.
    const [coins, chk] = await Promise.all([
      fetchCoinSearch(raw),
      isEvmAddress(raw) ? fetchCheck(raw) : Promise.resolve(null),
    ]);
    if (seq !== searchSeq) return; // застарілий запит — ігноруємо
    const html = buildSearchHtml(raw, coins);
    const strip = chk ? renderCheckStrip(chk) : "";
    box.innerHTML = (strip + html) || `<div class="px-4 py-3 text-sm text-slate-500">${t("search_no_results")}</div>`;
    box.classList.remove("hidden");
  }, 250);
}

export function closeSearch(): void {
  searchSeq++; // скасувати незавершені запити
  $("#search-results").classList.add("hidden");
  $<HTMLInputElement>("#search-input").value = "";
}

