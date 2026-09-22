/* ---------- Портфель гаманця: усі активи (Arkham-style) ---------- */

import type { Wallet, Holding, EntityType } from "./types";
import { t, lang, BEH_META } from "./i18n";
import { $, shortAddr, fmtUsd, fmtNum, fmtPrice, seededRand } from "./util";
import { state } from "./state";
import { escAttr, link } from "./ui";
import { getJSON } from "./api";
import { openFamilies } from "./graph";
import { BACKEND } from "./env";
import { priceOf } from "./prices";
import { networkById, explorerAddr, chainsForAddress } from "./networks";
import { arkhamLink, ENTITY_TYPE_CLS } from "./attribution";
import { clarityBadge, clarityLegendHtml, hasClarityTag } from "./clarity";
import { assetColor } from "./assets";
import { KIND_META, kindLabel } from "./render";
import { nav } from "./nav";




// Довідник активів для пошуку

function balanceSeries(seed: string, current: number, n = 120): number[] {
  const rnd = seededRand(seed);
  let v = current * (0.25 + rnd() * 0.4);
  const arr: number[] = [];
  for (let i = 0; i < n; i++) {
    const drift = (current - v) * 0.03;
    const vol = current * 0.06 * (rnd() - 0.5);
    v = Math.max(current * 0.05, v + drift + vol);
    arr.push(v);
  }
  arr[arr.length - 1] = current;
  return arr;
}

function areaChartSvg(series: number[], color: string): string {
  const W = 520, H = 120, pad = 4;
  const max = Math.max(...series), min = Math.min(...series);
  const xs = (i: number): number => pad + (i / (series.length - 1)) * (W - 2 * pad);
  const ys = (v: number): number => H - pad - ((v - min) / ((max - min) || 1)) * (H - 2 * pad);
  const line = series.map((v, i) => `${i ? "L" : "M"}${xs(i).toFixed(1)} ${ys(v).toFixed(1)}`).join(" ");
  const area = `${line} L${xs(series.length - 1).toFixed(1)} ${H} L${xs(0).toFixed(1)} ${H} Z`;
  return `<svg viewBox="0 0 ${W} ${H}" class="w-full h-28" preserveAspectRatio="none">
    <defs><linearGradient id="pfgrad" x1="0" y1="0" x2="0" y2="1"><stop offset="0%" stop-color="${color}" stop-opacity="0.35"/><stop offset="100%" stop-color="${color}" stop-opacity="0"/></linearGradient></defs>
    <path d="${area}" fill="url(#pfgrad)"/><path d="${line}" fill="none" stroke="${color}" stroke-width="2"/></svg>`;
}

// Дуга SVG-пончика (зовнішній/внутрішній радіуси, кути в радіанах)
function donutArc(cx: number, cy: number, rO: number, rI: number, a1: number, a2: number): string {
  const pt = (r: number, a: number): [number, number] => [cx + r * Math.cos(a), cy + r * Math.sin(a)];
  const large = a2 - a1 > Math.PI ? 1 : 0;
  const [x1, y1] = pt(rO, a1), [x2, y2] = pt(rO, a2), [x3, y3] = pt(rI, a2), [x4, y4] = pt(rI, a1);
  return `M${x1.toFixed(2)} ${y1.toFixed(2)} A${rO} ${rO} 0 ${large} 1 ${x2.toFixed(2)} ${y2.toFixed(2)} L${x3.toFixed(2)} ${y3.toFixed(2)} A${rI} ${rI} 0 ${large} 0 ${x4.toFixed(2)} ${y4.toFixed(2)} Z`;
}

// Інтерактивна діаграма-пончик розподілу активів (сегмент підсвічується при наведенні)
export function allocationDonutHtml(items: Array<{ label: string; value: number; color: string }>, total: number): string {
  const t0 = total || items.reduce((s, i) => s + i.value, 0) || 1;
  const top = items.filter((i) => i.value > 0).sort((a, b) => b.value - a.value);
  const shown = top.slice(0, 10);
  const restVal = top.slice(10).reduce((s, i) => s + i.value, 0);
  if (restVal > 0) shown.push({ label: t("pf_other"), value: restVal, color: "#64748b" });
  if (!shown.length) return `<div class="text-xs text-slate-500">—</div>`;
  const cx = 70, cy = 70, rO = 64, rI = 41;
  let a = -Math.PI / 2;
  const segs = shown.map((it, idx) => {
    const frac = it.value / t0;
    const a2 = a + Math.max(frac * Math.PI * 2, 0.0001);
    const d = donutArc(cx, cy, rO, rI, a, Math.min(a2, a + Math.PI * 2 - 0.0001));
    a = a2;
    return `<path d="${d}" fill="${it.color}" class="donut-seg" data-donut-seg="${idx}" data-label="${escAttr(it.label)}" data-val="${escAttr(fmtUsd(it.value))}" data-pct="${(frac * 100).toFixed(1)}"></path>`;
  }).join("");
  const legend = shown.map((it, idx) => `<button data-donut-seg="${idx}" class="flex items-center gap-1.5 text-[11px] text-left hover:text-accent transition">
      <span class="w-2.5 h-2.5 rounded-sm shrink-0" style="background:${it.color}"></span>
      <span class="truncate flex-1 min-w-0">${it.label}${hasClarityTag(it.label) ? clarityBadge(it.label, false) : ""}</span>
      <span class="text-slate-500 shrink-0">${((it.value / t0) * 100).toFixed(1)}%</span></button>`).join("");
  return `<div data-donut-root class="flex items-center gap-4">
    <div class="relative w-[140px] h-[140px] shrink-0">
      <svg viewBox="0 0 140 140" class="w-full h-full">${segs}</svg>
      <div data-donut-center data-clabel="${escAttr(t("pf_total"))}" data-cval="${escAttr(fmtUsd(t0))}" class="absolute inset-0 flex flex-col items-center justify-center text-center pointer-events-none px-6">
        <div class="text-[10px] text-slate-400 truncate max-w-full" data-donut-clabel>${t("pf_total")}</div>
        <div class="text-sm font-semibold truncate max-w-full" data-donut-cval>${fmtUsd(t0)}</div>
      </div>
    </div>
    <div class="flex-1 min-w-0 grid grid-cols-2 gap-x-3 gap-y-1">${legend}</div>
  </div>`;
}

let portfolioWallet: Wallet | null = null;

function buildPortfolioHtml(w: Wallet): string {
  const net0 = networkById(w.networks[0]);
  const rows = w.holdings
    .map((h) => ({ asset: h.asset, amount: h.amount, price: priceOf(h.asset), value: h.amount * priceOf(h.asset) }))
    .sort((a, b) => b.value - a.value);
  const total = rows.reduce((s, r) => s + r.value, 0) || 1;
  const series = balanceSeries(w.address, total);
  const change = ((total - series[0]) / (series[0] || 1)) * 100;
  const chart = areaChartSvg(series, "#3b82f6");

  const wevents = state.events.filter((e) => e.walletId === w.id).slice(0, 6);
  const connsMap = new Map<string, { label: string; type: EntityType; name: string; count: number }>();
  state.events.filter((e) => e.walletId === w.id).forEach((e) => {
    const k = e.counterpartyEntity.type === "unknown" ? `u:${e.counterparty}` : `e:${e.counterpartyEntity.name}`;
    const c = connsMap.get(k) || { label: e.counterpartyEntity.type === "unknown" ? shortAddr(e.counterparty) : e.counterpartyEntity.name, type: e.counterpartyEntity.type, name: e.counterpartyEntity.name, count: 0 };
    c.count += 1; connsMap.set(k, c);
  });
  const connList = [...connsMap.values()].sort((a, b) => b.count - a.count).slice(0, 8);

  const list = rows.map((r) => {
    const pct = (r.value / total) * 100;
    return `
      <div class="flex items-center gap-3 py-2 border-b border-edge/60">
        <span class="w-8 h-8 rounded-full grid place-items-center text-[10px] font-bold text-black shrink-0" style="background:${assetColor(r.asset)}">${r.asset.slice(0, 4)}</span>
        <div class="flex-1 min-w-0">
          <div class="flex justify-between gap-2 text-sm"><span class="font-medium">${r.asset}</span><span>${fmtUsd(r.value)}</span></div>
          <div class="flex justify-between gap-2 text-xs text-slate-400">
            <span>${fmtNum(r.amount, 4)} ${r.asset} · ${t("pf_price")} ${fmtPrice(r.price)}</span>
            <span>${pct.toFixed(1)}%</span>
          </div>
          <div class="mt-1 h-1 rounded bg-panel2"><div class="h-1 rounded" style="width:${pct.toFixed(1)}%;background:${assetColor(r.asset)}"></div></div>
        </div>
      </div>`;
  }).join("");

  return `
    <div>
      <div class="text-base font-semibold">${w.label || shortAddr(w.address)}</div>
      <div class="font-mono text-xs text-slate-300 break-all mt-0.5">${w.address}</div>
      ${w.valueUsd > 0 ? `<div class="text-sm mt-1"><span class="text-slate-400">${t("pf_total")}:</span> <span class="font-semibold text-slate-100">${fmtUsd(w.valueUsd)}</span>${w.realTokens != null ? ` <span class="text-slate-400">\u00b7 ${w.realTokens} ${t("dos_assets")}</span>` : ""} <span class="text-slate-400">\u00b7 ${w.networks.length} ${t("networks_word")}</span></div>` : ""}
      <div class="flex flex-wrap gap-3 mt-2 text-xs">
        ${link(explorerAddr(net0, w.address), t("btn_explorer"))}
        <a href="${arkhamLink(w.address)}" target="_blank" rel="noopener" class="text-purple-400 hover:underline">🔍 Arkham Intel →</a>
        ${/^0x[0-9a-fA-F]{40}$/.test(w.address) ? `<button data-money-trace="${escAttr(w.address)}" class="text-amber-400 hover:underline">💸 ${t("mt_btn")}</button>` : ""}
        ${/^0x[0-9a-fA-F]{40}$/.test(w.address) ? `<button data-report="${escAttr(w.address)}" class="text-fuchsia-400 hover:underline">📄 ${t("rep_btn")}</button>` : ""}
      </div>
      <div id="pf-family" class="mt-1"></div>
      <div id="pf-ml" class="mt-2"></div>
      <div id="pf-coord" class="mt-2"></div>
      <div id="pf-netflow" class="mt-2"></div>
      <div id="pf-report" class="mt-1"></div>
    </div>

    <div>
      <div class="text-xs text-slate-400 mb-2">${t("pf_allocation")}</div>
      <div id="pf-alloc">${allocationDonutHtml(rows.map((r) => ({ label: r.asset, value: r.value, color: assetColor(r.asset) })), total)}</div>
    </div>

    <div>
      <div class="flex items-center justify-between mb-1">
        <span class="text-xs text-slate-400">${t("pf_chart")}</span>
        <span class="text-xs ${change >= 0 ? "text-good" : "text-bad"}">${change >= 0 ? "▲" : "▼"} ${Math.abs(change).toFixed(1)}%</span>
      </div>
      ${chart}
    </div>

    <div>
      <div class="text-xs text-slate-400 mb-1">${t("pf_assets")}</div>
      ${list}
    </div>

    <div>
      <div class="text-xs text-slate-400 mb-1">🧠 ${t("behavior_title")}</div>
      <div id="behavior-intel" class="text-xs text-slate-400 space-y-0.5">${t("conc_loading")}</div>
    </div>

    ${/^0x[0-9a-fA-F]{40}$/.test(w.address) ? `
    <div id="wallet-score" class="bg-panel2 border border-edge rounded-xl p-3"><div class="text-[11px] text-slate-500">${t("ws_loading")}</div></div>
    <div>
      <div class="text-xs text-slate-400 mb-1">🔗 ${t("onchain_real")}</div>
      <div id="onchain-real" class="text-xs text-slate-400 space-y-0.5">${t("conc_loading")}</div>
    </div>
    <div>
      <div class="text-xs text-slate-400 mb-1">💼 ${t("wa_title")}</div>
      <div id="wallet-assets-full" class="text-xs text-slate-400 space-y-0.5">${t("conc_loading")}</div>
    </div>` : ""}

    <div>
      <div class="text-xs text-slate-400 mb-1">${t("g_connected_entities")} (${connsMap.size})</div>
      <div class="flex flex-wrap gap-1.5">
        ${connList.length ? connList.map((c) => c.type !== "unknown"
          ? `<button data-entity-name="${escAttr(c.name)}" data-entity-type="${c.type}" class="px-2 py-0.5 rounded-full text-xs ${ENTITY_TYPE_CLS[c.type]} hover:ring-1 hover:ring-accent">${c.label} · ${c.count}</button>`
          : `<span class="px-2 py-0.5 rounded-full text-xs ${ENTITY_TYPE_CLS["unknown"]}">${t("entity_unknown")} · ${c.count}</span>`).join("")
          : `<div class="text-xs text-slate-500">—</div>`}
      </div>
    </div>

    <div>
      <div class="text-xs text-slate-400 mb-1">${t("pf_recent_events")}</div>
      ${wevents.length ? wevents.map((e) => { const meta = KIND_META[e.kind]; const net = networkById(e.network); return `
        <div data-ev="${e.id}" class="flex items-center justify-between gap-2 py-1.5 border-b border-edge/60 cursor-pointer hover:bg-panel2/40 text-xs">
          <span class="${meta.cls}">${meta.icon} ${kindLabel(e.kind)}</span>
          <span class="${net.color}">${net.name}</span>
          <span class="text-slate-400 shrink-0">${fmtNum(e.amount, 3)} ${e.asset} · ${fmtUsd(e.valueUsd)}</span>
        </div>`; }).join("") : `<div class="text-xs text-slate-500">—</div>`}
    </div>

    <div class="grid grid-cols-2 gap-2">
      <button data-graph-wallet="${w.id}" class="bg-panel2 hover:border-accent border border-edge text-sm py-2 rounded-xl transition">🕸 ${t("graph_btn")}</button>
      <button data-tracer-wallet="${w.id}" class="bg-panel2 hover:border-accent border border-edge text-sm py-2 rounded-xl transition">${t("tracer_open")}</button>
    </div>
    <button data-cluster-wallet="${escAttr(w.address)}" class="w-full mt-2 bg-accent/15 hover:bg-accent/25 text-accent border border-accent/30 text-sm py-2 rounded-xl transition">🧩 ${t("cl_open")}</button>`;
}

const onchainCache: Record<string, { chain: string; balance: number | null }[]> = {};

async function loadOnchain(w: Wallet): Promise<void> {
  if (!BACKEND || !/^0x[0-9a-fA-F]{40}$/.test(w.address)) return;
  const el = document.querySelector<HTMLElement>("#onchain-real");
  if (!el) return;
  const render = (balances: { chain: string; balance: number | null }[]): void => {
    const rows = balances
      .filter((b) => b.balance && b.balance > 0)
      .map((b) => { const net = networkById(b.chain); return { net, bal: b.balance || 0, usd: (b.balance || 0) * priceOf(net.symbol) }; })
      .sort((a, b) => b.usd - a.usd);
    const total = rows.reduce((s, r) => s + r.usd, 0);
    el.innerHTML = `<div class="text-slate-300 mb-1">${t("onchain_total")}: <span class="font-medium">${fmtUsd(total)}</span></div>` +
      (rows.length ? rows.map((r) => `<div class="flex justify-between gap-2"><span class="${r.net.color}">${r.net.name}</span><span class="text-slate-400">${fmtNum(r.bal, 4)} ${r.net.symbol} · ${fmtUsd(r.usd)}</span></div>`).join("") : `<div class="text-slate-500">—</div>`);
  };
  if (onchainCache[w.address]) { render(onchainCache[w.address]); return; }
  try {
    const d = await getJSON<{ balances?: Array<{ chain: string; balance: number | null }> }>("/api/evm-balances", { address: w.address });
    onchainCache[w.address] = d.balances || [];
    render(onchainCache[w.address]);
  } catch { el.textContent = t("conc_err"); }
}

async function loadIntel(w: Wallet): Promise<void> {
  if (!BACKEND) return;
  const el = document.querySelector<HTMLElement>("#behavior-intel");
  if (!el) return;
  try {
    const d = await getJSON<{ behavior: string; netFlowUsd: number; whale?: boolean; ml?: { label: string; confidence: number } | null; inUsd: number; outUsd: number; counterparties: number; txCount: number }>(`/api/wallet-intel/${w.id}`);
    const bm = BEH_META[d.behavior] || BEH_META.trader;
    const netCls = d.netFlowUsd >= 0 ? "text-good" : "text-bad";
    el.innerHTML = `
      <div class="flex items-center gap-1.5 flex-wrap mb-1">
        ${d.whale ? `<span class="px-2 py-0.5 rounded-full text-[10px] bg-blue-400/10 text-blue-400">🐋 ${t("beh_whale")}</span>` : ""}
        <span class="px-2 py-0.5 rounded-full text-[10px] ${bm.cls}">${bm.icon} ${t(bm.key)}</span>
        ${d.ml ? `<span class="px-2 py-0.5 rounded-full text-[10px] bg-accent/15 text-accent" title="${t("ml_title")}">🧠 ML: ${t("ml_" + d.ml.label)} ${d.ml.confidence}%</span>` : ""}
      </div>
      <div class="flex justify-between gap-2"><span>${t("beh_netflow")}</span><span class="${netCls}">${d.netFlowUsd >= 0 ? "+" : ""}${fmtUsd(d.netFlowUsd)}</span></div>
      <div class="flex justify-between gap-2"><span>${t("beh_in")} / ${t("beh_out")}</span><span>${fmtUsd(d.inUsd)} / ${fmtUsd(d.outUsd)}</span></div>
      <div class="flex justify-between gap-2"><span>${t("beh_cps")} · ${t("beh_tx")}</span><span>${d.counterparties} · ${d.txCount}</span></div>`;
  } catch { el.textContent = "—"; }
}

export function openPortfolioFor(w: Wallet): void {
  portfolioWallet = w;
  w.valueUsd = typeof w.realUsd === "number" ? w.realUsd : w.holdings.reduce((s, h) => s + h.amount * priceOf(h.asset), 0);
  $("#portfolio-body").innerHTML = buildPortfolioHtml(w);
  const modal = $("#portfolio-modal");
  modal.classList.remove("hidden"); modal.classList.add("flex");
  void loadOnchain(w);
  void loadIntel(w);
  void loadWalletAssets(w);
  void loadWalletScore(w);
  void loadWalletNetflow(w);
  void loadWalletPrediction(w);
  void loadCoordination(w);
  void loadWalletFamily(w);
}

// ЯКІСТЬ СИГНАЛУ: впевненість координації для БУДЬ-ЯКОГО гаманця (не лише диригента).
async function loadWalletFamily(w: Wallet): Promise<void> {
  if (!BACKEND || !/^0x[0-9a-fA-F]{40}$/.test(w.address)) return;
  const el = document.getElementById("pf-family");
  if (!el) return;
  try {
    const d = await getJSON<{ inFamily: boolean; family?: { size: number; volumeUsd: number; topEmission?: Array<{ symbol: string; emissionPct: number }> } }>("/api/wallet-family", { address: w.address });
    if (!d.inFamily || !d.family || !document.getElementById("pf-family")) { el.innerHTML = ""; return; }
    const f = d.family;
    const vol = f.volumeUsd >= 1e6 ? `$${(f.volumeUsd / 1e6).toFixed(1)}M` : `$${(f.volumeUsd / 1e3).toFixed(0)}K`;
    const em = (f.topEmission || []).slice(0, 3).map((e) => `${e.symbol} ${e.emissionPct}%`).join(" \u00b7 ");
    el.innerHTML = `<button data-open-families class="text-[11px] px-2 py-1 rounded bg-fuchsia-500/15 text-fuchsia-300 border border-fuchsia-500/30 hover:bg-fuchsia-500/25 text-left">\ud83d\udd78 ${t("wf_label")}: <b>${f.size}</b> ${t("fam_wallets")} \u00b7 ${vol}${em ? ` \u00b7 ${t("fam_emission")} ${em}` : ""}</button>`;
    el.querySelector("[data-open-families]")?.addEventListener("click", () => void openFamilies());
  } catch { if (document.getElementById("pf-family")) el.innerHTML = ""; }
}

async function loadCoordination(w: Wallet): Promise<void> {
  if (!BACKEND || !/^0x[0-9a-fA-F]{40}$/.test(w.address)) return;
  const el = document.getElementById("pf-coord"); if (!el) return;
  el.innerHTML = `<div class="text-[11px] text-slate-500">${t("coord_loading")}</div>`;
  try {
    const d = await getJSON<{ confidence: number; labelKey: string; evidence: { funded: number; totalFundedUsd: number; synchronicity: number; reach: number; capital: number; consistency: number } }>("/api/coordination", { address: w.address });
    if (!document.getElementById("pf-coord")) return;
    const col = d.confidence >= 70 ? "#34d399" : d.confidence >= 45 ? "#5b9dff" : d.confidence >= 20 ? "#fb923c" : "#64748b";
    const ev = d.evidence;
    const bar = (label: string, v: number) => `<div class="flex items-center gap-2 text-[10px]"><span class="w-24 text-slate-400 shrink-0">${label}</span><div class="flex-1 h-1.5 rounded bg-panel"><div class="h-1.5 rounded" style="width:${v}%;background:${col}"></div></div><span class="w-8 text-right text-slate-400">${v}%</span></div>`;
    el.innerHTML = `<div class="bg-panel2 border rounded-xl p-3" style="border-color:${col}55">
      <div class="flex items-center justify-between mb-1.5">
        <span class="text-sm font-semibold">🎯 ${t("coord_title")}</span>
        <span class="text-xs px-2 py-0.5 rounded-full" style="background:${col}22;color:${col}">${t(d.labelKey)} · ${d.confidence}%</span>
      </div>
      <div class="text-[10px] text-slate-400 mb-2">${t("coord_funded")}: <b class="text-slate-200">${ev.funded}</b> · ${fmtUsd(ev.totalFundedUsd)}</div>
      ${bar(t("coord_reach"), ev.reach)}${bar(t("coord_capital"), ev.capital)}${bar(t("coord_sync"), ev.synchronicity)}${bar(t("coord_consistency"), ev.consistency)}
      <div class="text-[9px] text-slate-500 mt-1.5">${t("coord_note")}</div>
    </div>`;
  } catch { if (document.getElementById("pf-coord")) el.innerHTML = ""; }
}

// ШВИДКИЙ ML-прогноз окремим бейджем (не чекає повільного скорингу) — щоб прогноз було видно одразу.
let _mlStatus: { acc: number | null; cvAcc: number | null; cvAuc: number | null; samples: number } | null = null;
async function getMlStatus(): Promise<typeof _mlStatus> { if (_mlStatus) return _mlStatus; try { _mlStatus = await getJSON<{ acc: number | null; cvAcc: number | null; cvAuc: number | null; samples: number }>("/api/ml-status"); } catch { /* ignore */ } return _mlStatus; }

async function loadWalletPrediction(w: Wallet): Promise<void> {
  if (!BACKEND || !/^0x[0-9a-fA-F]{40}$/.test(w.address)) return;
  const el = document.getElementById("pf-ml");
  if (!el) return;
  el.innerHTML = `<span class="text-[11px] text-slate-500">🤖 ${t("rep_gen")}</span>`;
  try {
    const d = await getJSON<{ available: boolean; prob?: number }>("/api/sm-predict", { address: w.address });
    if (!d.available || typeof d.prob !== "number") { el.innerHTML = ""; return; }
    const ms = await getMlStatus();
    // Показуємо AUC, а не точність. Точність тут — метрика-обманка: позитивів лише 12%, тож
    // «завжди НІ» дало б 88%, а модель має 66% CV — гірше, ніж не робити нічого. Але сигнал у неї
    // Є: cvAuc 0.72 проти 0.5 у монетки. Саме AUC відповідає на питання, яке ставить користувач
    // («чи варто цьому гаманцю вірити більше, ніж іншому»), і не залежить від перекосу класів.
    const accNote = ms?.cvAuc != null ? ` · ${t("ml_auc")} ${ms.cvAuc.toFixed(2)}${t("ml_auc_scale")}` : ` · ${t("ml_calibrating")}`;
    const col = d.prob >= 60 ? "bg-good/15 text-good border-good/30" : d.prob <= 35 ? "bg-bad/15 text-bad border-bad/30" : "bg-yellow-500/15 text-yellow-400 border-yellow-500/30";
    el.innerHTML = `<span class="inline-flex items-center gap-1 text-xs px-2.5 py-1 rounded-full border ${col}" title="${t("ml_acc_title")}">🤖 ${t("ws_ai")}: <b>${d.prob}%</b> <span class="opacity-70 text-[10px]">${t("ws_ai_note")}${accNote}</span></span>`;
  } catch { el.innerHTML = ""; }
}

// Аналітичний звіт + прогноз гаманця (синтез + пояснюваний forecast) — рендериться в картці.
interface TakeProfit { sym: string; costUsd: number; currentUsd: number; multiple: number; tier: string; sellPct: number; }
interface WalletRep { volume: number; assetCount: number; score: number | null; roi: number | null; ml: number | null; isConductor: boolean; verdict: string; summary: string[]; forecast: { direction: string; confidence: number; reasons: Array<{ sign: string; text: string }> }; holdings: Array<{ symbol: string; valueUsd: number }>; takeProfit?: TakeProfit[]; }
export async function loadWalletReport(address: string): Promise<void> {
  const el = document.getElementById("pf-report");
  if (!el) return;
  el.innerHTML = `<div class="text-[11px] text-slate-500">${t("rep_gen")}</div>`;
  try {
    const d = await getJSON<WalletRep>("/api/report", { address });
    const dir = d.forecast.direction;
    const col = dir === "bullish" ? "text-good" : dir === "bearish" ? "text-bad" : "text-yellow-400";
    const bg = dir === "bullish" ? "bg-good/10 border-good/30" : dir === "bearish" ? "bg-bad/10 border-bad/30" : "bg-yellow-500/10 border-yellow-500/30";
    const signCol = (s: string) => s === "bull" ? "text-good" : s === "bear" ? "text-bad" : "text-slate-400";
    const fUsd = (n: number) => n >= 1e6 ? "$" + (n / 1e6).toFixed(1) + "M" : n >= 1e3 ? "$" + (n / 1e3).toFixed(0) + "k" : "$" + Math.round(n);
    const tp = (d.takeProfit || []).filter((x) => x.tier !== "hold");
    const tpHtml = tp.length ? `
      <div class="text-[10px] text-slate-400 mt-2 mb-1">💰 ${t("rep_tp")} (+200%):</div>
      <div class="text-[11px] space-y-0.5">${tp.map((x) => x.tier === "take"
        ? `<div class="text-good">▲ <b>${x.sym} ${x.multiple}x</b> — ${t("rep_tp_take")} ~${x.sellPct}% <span class="text-slate-500">(${t("rep_tp_body")} ${fUsd(x.costUsd)} → ${fUsd(x.currentUsd)})</span></div>`
        : `<div class="text-yellow-400">• <b>${x.sym} ${x.multiple}x</b> — ${t("rep_tp_approach")} <span class="text-slate-500">(${fUsd(x.currentUsd)})</span></div>`).join("")}</div>` : "";
    el.innerHTML = `<div class="rounded-xl border ${bg} p-3 mt-1">
      <div class="flex items-center justify-between mb-1"><span class="text-sm font-bold ${col}">${d.verdict}</span><span class="text-[11px] text-slate-400">${t("rep_conf")} ${d.forecast.confidence}%</span></div>
      <div class="text-[11px] text-slate-300 space-y-0.5 mb-2">${d.summary.map((s) => `<div>• ${s}</div>`).join("")}</div>
      <div class="text-[10px] text-slate-400 mb-1">${t("rep_why")}:</div>
      <div class="text-[11px] space-y-0.5">${d.forecast.reasons.map((r) => `<div class="${signCol(r.sign)}">${r.sign === "bull" ? "▲" : r.sign === "bear" ? "▼" : "•"} ${r.text}</div>`).join("") || `<div class="text-slate-500">${t("rep_nodata")}</div>`}</div>
      ${tpHtml}
      <button id="rep-export" class="text-[11px] text-accent hover:underline mt-2">📥 ${t("rep_export")}</button>
      <div class="text-[9px] text-slate-600 mt-1">${t("rep_note")}</div>
    </div>`;
    el.querySelector("#rep-export")?.addEventListener("click", () => downloadDossier(address, d));
  } catch { el.innerHTML = `<div class="text-[11px] text-bad">${t("conc_err")}</div>`; }
}

// Завантаження ДОСЬЄ: самодостатній HTML (відкривається в браузері, друкується в PDF).
function downloadDossier(address: string, d: WalletRep): void {
  const f = (n: number): string => n >= 1e9 ? "$" + (n / 1e9).toFixed(2) + "B" : n >= 1e6 ? "$" + (n / 1e6).toFixed(1) + "M" : n >= 1e3 ? "$" + (n / 1e3).toFixed(0) + "k" : "$" + Math.round(n);
  const now = new Date().toISOString().slice(0, 10);
  const dcl = d.forecast.direction === "bullish" ? "bull" : d.forecast.direction === "bearish" ? "bear" : "neu";
  const html = `<!doctype html><html lang="${lang}"><head><meta charset="utf-8"><title>${t("dos_title")} ${address}</title>
<style>body{font-family:system-ui,Segoe UI,sans-serif;max-width:820px;margin:2rem auto;padding:0 1.2rem;color:#1e293b;line-height:1.5}h1{font-size:1.5rem;margin-bottom:.2rem}h2{font-size:1.05rem;border-bottom:2px solid #e2e8f0;padding-bottom:5px;margin-top:1.6rem}code{background:#f1f5f9;padding:2px 6px;border-radius:4px;font-size:.85rem}.verdict{font-size:1.2rem;font-weight:700;padding:.6rem 1rem;border-radius:10px;display:inline-block}.bull{color:#16a34a;background:#f0fdf4}.bear{color:#dc2626;background:#fef2f2}.neu{color:#ca8a04;background:#fefce8}table{border-collapse:collapse;width:100%;font-size:.9rem}td{padding:4px 8px;border-bottom:1px solid #eef2f7}td.r{text-align:right;font-variant-numeric:tabular-nums}ul{margin:.3rem 0}.muted{color:#94a3b8;font-size:.78rem;margin-top:2rem}.tbull{color:#16a34a}.tbear{color:#dc2626}</style></head><body>
<h1>🛰 Wallet Radar — ${t("dos_title")}</h1><p><code>${address}</code> · ${now}</p>
${d.isConductor ? `<p>🎼 <b>${t("cond_h").replace(/ \(.*/, "")}</b></p>` : ""}
<h2>${t("dos_verdict")}</h2><p class="verdict ${dcl}">${d.verdict} · ${t("rep_conf")} ${d.forecast.confidence}%</p>
<h2>${t("dos_summary")}</h2><ul>${d.summary.map((s) => `<li>${s}</li>`).join("")}</ul>
<h2>${t("rep_why")}</h2><ul>${d.forecast.reasons.map((r) => `<li class="${r.sign === "bull" ? "tbull" : r.sign === "bear" ? "tbear" : ""}">${r.text}</li>`).join("") || "<li>—</li>"}</ul>
<h2>${t("dos_metrics")}</h2><table><tr><td>${t("dos_volume")}</td><td class="r">${f(d.volume)}</td></tr><tr><td>${t("dos_assets")}</td><td class="r">${d.assetCount}</td></tr><tr><td>${t("dos_score")}</td><td class="r">${d.score ?? "—"}</td></tr><tr><td>ROI</td><td class="r">${d.roi ?? "—"}%</td></tr><tr><td>ML smart-money</td><td class="r">${d.ml ?? "—"}%</td></tr></table>
<h2>${t("dos_holdings")} (${d.holdings.length})</h2><table>${d.holdings.map((h) => `<tr><td>${h.symbol}</td><td class="r">${f(h.valueUsd)}</td></tr>`).join("")}</table>
<p class="muted">${t("rep_note")} · ${t("dos_generated")} · ${now}</p></body></html>`;
  const url = URL.createObjectURL(new Blob([html], { type: "text/html" }));
  const a = document.createElement("a");
  a.href = url; a.download = `dossier-${address.slice(0, 10)}-${now}.html`;
  a.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

// Бейдж вердикту: що гаманець у підсумку накопичує/скидає на біржах (швидкий прямий нетфлоу).
async function loadWalletNetflow(w: Wallet): Promise<void> {
  if (!BACKEND || !/^0x[0-9a-fA-F]{40}$/.test(w.address)) return;
  const el = document.getElementById("pf-netflow");
  if (!el) return;
  try {
    const d = await getJSON<{ netflows: Array<{ asset: string; net: number }> }>("/api/wallet-netflow", { address: w.address });
    const nf = d.netflows || [];
    if (!nf.length) { el.innerHTML = ""; return; }
    el.innerHTML = nf.slice(0, 4).map((x) => { const pos = x.net >= 0; return `<span class="inline-flex items-center gap-1 text-[11px] px-2 py-0.5 rounded-full mr-1 mb-1 ${pos ? "bg-good/15 text-good border border-good/30" : "bg-bad/15 text-bad border border-bad/30"}">${pos ? "📈 " + t("mt_accum") : "📉 " + t("mt_dump")} <b>${x.asset}</b> ${pos ? "+" : "−"}${fmtUsd(Math.abs(x.net))}</span>`; }).join("");
  } catch { el.innerHTML = ""; }
}

interface WalletScore { score: number; roi: number | null; winRate: number | null; realizedUsd: number; investedUsd: number; positions: number; closed: number; wins: number; losses: number; holdDays: number | null; best: Array<{ sym: string; pnl: number }>; worst: Array<{ sym: string; pnl: number }>; }

// Скоринг гаманця: win-rate / ROI / realized PnL / час утримання (ончейн-історія, історичні ціни).
async function loadWalletScore(w: Wallet): Promise<void> {
  if (!BACKEND || !/^0x[0-9a-fA-F]{40}$/.test(w.address)) return;
  const el = document.getElementById("wallet-score");
  if (!el) return;
  try {
    const [d, pred] = await Promise.all([
      getJSON<WalletScore>("/api/wallet-score", { address: w.address }),
      getJSON<{ available: boolean; prob?: number }>("/api/sm-predict", { address: w.address }).catch((): { available: boolean; prob?: number } => ({ available: false })),
    ]);
    if (typeof d.score !== "number") { el.innerHTML = `<div class="text-[11px] text-slate-500">${t("ws_none")}</div>`; return; }
    const aiLine = pred?.available && typeof pred.prob === "number"
      ? `<div class="mt-1.5 pt-1.5 border-t border-edge/60 text-[11px]">🤖 <span class="text-slate-400">${t("ws_ai")}:</span> <b class="${pred.prob >= 60 ? "text-good" : pred.prob <= 35 ? "text-bad" : "text-yellow-400"}">${pred.prob}%</b> <span class="text-slate-500">${t("ws_ai_note")}</span></div>`
      : "";
    const col = d.score >= 65 ? "text-good" : d.score <= 40 ? "text-bad" : "text-yellow-400";
    const ring = d.score >= 65 ? "#22c55e" : d.score <= 40 ? "#ef4444" : "#eab308";
    const pnlCol = d.realizedUsd >= 0 ? "text-good" : "text-bad";
    const stat = (label: string, val: string, c = "text-slate-200"): string => `<div class="flex flex-col"><span class="text-[10px] text-slate-500">${label}</span><span class="text-xs font-medium ${c}">${val}</span></div>`;
    el.innerHTML = `
      <div class="flex items-center gap-3">
        <div class="relative w-14 h-14 shrink-0 rounded-full grid place-items-center" style="background:conic-gradient(${ring} ${d.score}%, #1e293b ${d.score}%)">
          <div class="absolute inset-[3px] rounded-full bg-panel2 grid place-items-center"><span class="text-sm font-bold ${col}">${d.score}</span></div>
        </div>
        <div class="flex-1">
          <div class="text-xs font-medium mb-1">🧠 ${t("ws_title")}</div>
          <div class="grid grid-cols-4 gap-2">
            ${stat(t("ws_roi"), d.roi != null ? `${d.roi > 0 ? "+" : ""}${d.roi}%` : "—", d.roi != null && d.roi >= 0 ? "text-good" : "text-bad")}
            ${stat(t("ws_winrate"), d.winRate != null ? `${d.winRate}%` : "—")}
            ${stat(t("ws_pnl"), fmtUsd(d.realizedUsd), pnlCol)}
            ${stat(t("ws_hold"), d.holdDays != null ? `${d.holdDays}${t("ws_days")}` : "—")}
          </div>
        </div>
      </div>
      <div class="text-[10px] text-slate-500 mt-1.5">${d.closed} ${t("ws_closed")} · ${d.positions} ${t("ws_positions")}${d.best[0] ? ` · ${t("ws_best")}: ${d.best[0].sym} ${fmtUsd(d.best[0].pnl)}` : ""}${d.worst[0] && d.worst[0].pnl < 0 ? ` · ${t("ws_worst")}: ${d.worst[0].sym} ${fmtUsd(d.worst[0].pnl)}` : ""}</div>
      ${aiLine}
      <div class="text-[10px] text-slate-600 mt-0.5">${t("ws_note")}</div>`;
  } catch { el.innerHTML = `<div class="text-[11px] text-slate-500">${t("ws_none")}</div>`; }
}

async function loadWalletAssets(w: Wallet): Promise<void> {
  if (!BACKEND || !/^0x[0-9a-fA-F]{40}$/.test(w.address)) return;
  const el = document.querySelector<HTMLElement>("#wallet-assets-full");
  if (!el) return;
  try {
    const d = await getJSON<{ totalUsd: number; txCount: number; tokens: Array<{ symbol: string; name: string; balance: number; valueUsd: number }>; flow: { inUsd: number; outUsd: number; txTotal: number; chains: number; timespanDays: number } | null }>("/api/wallet-assets", { address: w.address });
    const vol = d.flow ? `<div class="flex flex-wrap gap-x-4 gap-y-0.5 mb-1.5 text-[11px]">
        <span>↘ ${t("wa_in")}: <span class="text-good">${fmtUsd(d.flow.inUsd)}</span></span>
        <span>↗ ${t("wa_out")}: <span class="text-bad">${fmtUsd(d.flow.outUsd)}</span></span>
        <span>${t("wa_txs")}: ${d.flow.txTotal}</span>
        <span>${t("wa_chains")}: ${d.flow.chains}</span>
        ${d.flow.timespanDays ? `<span>${t("wa_period")}: ~${Math.round(d.flow.timespanDays)} ${t("wa_days")}</span>` : ""}
      </div>` : "";
    const toks = d.tokens.length ? d.tokens.slice(0, 12).map((tok) => `<div class="flex justify-between gap-2"><span class="flex items-center min-w-0 max-w-[60%]"><span class="truncate">${tok.symbol}</span>${clarityBadge(tok.symbol)}</span><span class="text-slate-400 shrink-0">${fmtNum(tok.balance, 2)}${tok.valueUsd ? ` · ${fmtUsd(tok.valueUsd)}` : ""}</span></div>`).join("") : `<div class="text-slate-500">—</div>`;
    el.innerHTML = `<div class="text-slate-300 mb-1">${t("wa_total")}: <span class="font-medium">${fmtUsd(d.totalUsd)}</span> · ${d.tokens.length} ${t("wa_assets")} · ${d.txCount} tx</div>${vol}${toks}${clarityLegendHtml()}`;
    // Живимо діаграму розподілу РЕАЛЬНИМИ даними (Ethplorer) + коректна загальна вартість
    const alloc = document.getElementById("pf-alloc");
    const priced = d.tokens.filter((tok) => tok.valueUsd > 0);
    if (alloc && d.totalUsd > 0 && priced.length) {
      alloc.innerHTML = allocationDonutHtml(priced.map((tok) => ({ label: tok.symbol, value: tok.valueUsd, color: assetColor(tok.symbol) })), d.totalUsd);
    }
  } catch { el.textContent = t("conc_err"); }
}

// Глобальне відкриття Arkham-картки для БУДЬ-ЯКОЇ адреси (клік звідусіль).
export function openWalletAddress(address: string, label?: string): void {
  const a = (address || "").trim();
  if (!a) return;
  const existing = state.wallets.find((x) => x.address.toLowerCase() === a.toLowerCase());
  const w: Wallet = existing || { id: "ext-" + a.slice(2, 12), address: a, networks: chainsForAddress(a), label: label || "", holdings: [], valueUsd: 0 };
  // закрити будь-яке вікно-джерело, щоб портфель (Arkham-картка) був зверху й чисто
  nav.closeOverlays();
  openPortfolioFor(w);
}

export function openPortfolio(wid: string): void {
  const w = state.wallets.find((x) => x.id === wid);
  if (w) openPortfolioFor(w);
}

export function closePortfolio(): void {
  portfolioWallet = null;
  const modal = $("#portfolio-modal");
  modal.classList.add("hidden"); modal.classList.remove("flex");
}
/** Синтетичний портфель для довільної (нетрекнутої) адреси з пошуку. */
export function syntheticWallet(addr: string): Wallet {
  const rnd = seededRand(addr);
  const chains = chainsForAddress(addr);
  const primary = chains[0];
  const pool = primary === "solana" ? ["SOL", "USDC"]
    : primary === "tron" ? ["TRX", "USDT"]
    : primary === "dogecoin" ? ["DOGE"]
    : primary === "bitcoin" ? ["BTC", "WBTC"]
    : ["ETH", "USDT", "USDC", "WBTC", "ARB", "LINK", "UNI", "AAVE"];
  const k = Math.min(pool.length, 2 + Math.floor(rnd() * 4));
  const chosen = [...pool].sort(() => rnd() - 0.5).slice(0, k);
  const holdings: Holding[] = chosen.map((asset) => {
    const p = priceOf(asset) || 1;
    return { asset, amount: +((1_000 + rnd() * 800_000) / p).toFixed(4) };
  });
  return { id: `search:${addr}`, address: addr, networks: chains, holdings, valueUsd: 0 };
}

/** Поточний відкритий у портфелі гаманець (для графа/трасування). */
export function getPortfolioWallet(): Wallet | null { return portfolioWallet; }

/** Перемалювати портфель під поточний гаманець (після зміни мови/цін). */
export function refreshPortfolio(): void {
  if (portfolioWallet) openPortfolioFor(portfolioWallet);
}
