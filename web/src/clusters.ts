/* ---------- Кластеризація гаманців + аналітика груп + кінцева ціль ---------- */

import { t } from "./i18n";
import { shortAddr, fmtUsd, fmtNum } from "./util";
import { escAttr } from "./ui";
import { getJSON } from "./api";
import { state } from "./state";

interface ClCard { category: string; count: number; inUsd: number; outUsd: number; txIn: number; txOut: number; avgBalanceUsd: number | null; top: Array<{ addr: string; name: string | null; inUsd: number; outUsd: number; tx: number; chains: string[] }>; }
interface ClResp { address: string; type: string; chains: string[]; counterparties: number; coverage: number; period: { first: number | null; last: number | null }; timeline: Array<{ period: string; inUsd: number; outUsd: number; txIn: number; txOut: number }>; clusters: ClCard[]; intent: { verdict: string; verdictPct: number; basis: string; buyUsd: number; sellUsd: number; netCexUsd: number; netFlowUsd: number; stance: string; outByCat: Record<string, number>; inByCat: Record<string, number>; totalOutUsd: number; totalInUsd: number }; }
const CAT_COLOR: Record<string, string> = { cex: "#eab308", dex: "#38bdf8", bridge: "#a855f7", defi: "#22d3ee", eoa: "#64748b" };

function clBar(byCat: Record<string, number>): string {
  const total = Object.values(byCat).reduce((a, b) => a + b, 0);
  if (!total) return "";
  const order = ["cex", "dex", "bridge", "defi", "eoa"];
  return `<div class="flex h-2.5 rounded-full overflow-hidden bg-panel2 mt-2">` +
    order.filter((c) => byCat[c] > 0).map((c) => `<div style="width:${(byCat[c] / total * 100).toFixed(1)}%;background:${CAT_COLOR[c]}" class="h-full"></div>`).join("") + `</div>`;
}

function clTimeline(tl: Array<{ period: string; inUsd: number; outUsd: number; txIn: number; txOut: number }>): string {
  if (!tl?.length) return "";
  const max = Math.max(...tl.map((m) => m.inUsd + m.outUsd), 1);
  const bars = tl.map((m) => {
    const tot = m.inUsd + m.outUsd;
    const h = Math.max(2, Math.round(Math.log10(tot + 1) / Math.log10(max + 1) * 42));
    const col = m.outUsd > m.inUsd ? "#ef4444" : "#22c55e";
    return `<div title="${m.period}: ↘$${fmtNum(m.inUsd)} / ↗$${fmtNum(m.outUsd)} · ${m.txIn + m.txOut} tx" style="height:${h}px;background:${col}" class="flex-1 min-w-[2px] rounded-sm self-end opacity-80 hover:opacity-100"></div>`;
  }).join("");
  return `<div class="flex items-end gap-[1px] h-[46px]">${bars}</div>
    <div class="flex justify-between text-[10px] text-slate-500 mt-1"><span>${tl[0].period}</span><span>${tl[tl.length - 1].period}</span></div>`;
}

function renderClusters(d: ClResp): string {
  const v = d.intent?.verdict || "hold";
  const pct = d.intent?.verdictPct || 0;
  const head = `
    <div class="rounded-xl border border-accent/30 bg-accent/10 p-4 mb-3">
      <div class="text-xs text-slate-400 mb-1">${t("cl_goal")}</div>
      <div class="text-lg font-bold text-accent">${t("cl_v_" + v)}${pct ? ` · ${pct}%` : ""}</div>
      <div class="text-[11px] text-slate-400 mt-1">${d.counterparties} ${t("cl_cps")} · ${d.chains.length} ${t("networks_word")} · ${t("cl_basis")}: ${d.intent.basis === "usd" ? "USD" : "tx"} · ${t("cl_coverage")}: ${d.coverage}%</div>
      <div class="flex flex-wrap gap-1.5 mt-2 text-[11px]">
        <span class="px-2 py-0.5 rounded bg-good/10 text-good">${t("cl_buy")}: ${fmtUsd(d.intent.buyUsd)}</span>
        <span class="px-2 py-0.5 rounded bg-bad/10 text-bad">${t("cl_sell")}: ${fmtUsd(d.intent.sellUsd)}</span>
        <span class="px-2 py-0.5 rounded bg-panel2 text-slate-300">${t("cl_stance")}: ${t("cl_" + d.intent.stance)} (${d.intent.netFlowUsd >= 0 ? "+" : ""}${fmtUsd(d.intent.netFlowUsd)})</span>
      </div>
      ${clBar(d.intent.outByCat)}
    </div>`;
  const ym = (ms: number | null): string => { if (!ms) return "—"; const dt = new Date(ms); return `${dt.getUTCFullYear()}-${String(dt.getUTCMonth() + 1).padStart(2, "0")}`; };
  const timelineCard = d.timeline?.length ? `<div class="rounded-xl border border-edge bg-panel2 p-3 mb-3">
      <div class="flex justify-between items-center mb-1.5"><span class="text-xs font-medium">${t("cl_timeline")}</span><span class="text-[11px] text-slate-500">${t("cl_period")}: ${ym(d.period.first)} → ${ym(d.period.last)}</span></div>
      ${clTimeline(d.timeline)}</div>` : "";
  const cards = d.clusters.slice().sort((a, b) => (b.outUsd + b.inUsd + b.txOut + b.txIn) - (a.outUsd + a.inUsd + a.txOut + a.txIn)).map((c) => `
    <div class="rounded-xl border border-edge bg-panel2 p-3 mb-2">
      <div class="flex items-center justify-between mb-1">
        <div class="flex items-center gap-2"><span class="w-2.5 h-2.5 rounded-full" style="background:${CAT_COLOR[c.category]}"></span><span class="font-medium text-sm">${t("cl_cat_" + c.category)}</span><span class="text-[11px] text-slate-500">${c.count} ${t("cl_addr")}</span></div>
        ${c.avgBalanceUsd != null ? `<span class="text-[11px] text-slate-400">${t("cl_avgbal")}: <span class="text-slate-200">${fmtUsd(c.avgBalanceUsd)}</span></span>` : ""}
      </div>
      <div class="flex gap-4 text-[11px] text-slate-400 mb-1.5">
        <span>↘ ${t("cl_in")}: <span class="text-good">${fmtUsd(c.inUsd)}</span> · ${c.txIn} tx</span>
        <span>↗ ${t("cl_out")}: <span class="text-bad">${fmtUsd(c.outUsd)}</span> · ${c.txOut} tx</span>
      </div>
      ${c.top.slice(0, 4).map((m) => `<div class="flex justify-between text-[11px] text-slate-500"><span data-wallet-addr="${escAttr(m.addr)}" class="font-mono truncate max-w-[60%] cursor-pointer hover:text-accent">${m.name || shortAddr(m.addr)}</span><span>${m.tx} tx · ${fmtUsd(m.inUsd + m.outUsd)}</span></div>`).join("")}
    </div>`).join("");
  const twohop = d.type === "evm" ? `<button id="cl-2hop" data-addr="${escAttr(d.address)}" class="w-full mb-2 text-xs py-2 rounded-xl border border-edge bg-panel2 hover:border-accent transition">🔗 ${t("cl_2hop_btn")}</button><div id="cl-2hop-result" class="mb-2"></div>` : "";
  return head + timelineCard + twohop + cards;
}

function render2hop(d: { neighbors: number; clusters: Array<{ size: number; members: Array<{ addr: string; name: string | null; count: number }> }> }): string {
  if (!d.clusters?.length) return `<div class="text-[11px] text-slate-500 py-2">${t("cl_2hop_none")} · ${d.neighbors} ${t("cl_cps")}</div>`;
  return d.clusters.map((c, i) => `<div class="rounded-lg border border-accent/20 bg-accent/5 p-2 mb-1.5">
      <div class="text-xs font-medium text-accent mb-1">🔗 ${t("cl_2hop_group")} ${i + 1} · ${c.size}</div>
      ${c.members.map((m) => `<div class="flex justify-between text-[11px] text-slate-400"><span data-wallet-addr="${escAttr(m.addr)}" class="font-mono truncate max-w-[65%] cursor-pointer hover:text-accent">${m.name || shortAddr(m.addr)}</span><span>${m.count} tx</span></div>`).join("")}
    </div>`).join("");
}

export async function openClusters(address?: string): Promise<void> {
  let modal = document.getElementById("cluster-modal");
  if (!modal) {
    modal = document.createElement("div");
    modal.id = "cluster-modal";
    modal.className = "fixed inset-0 z-50 flex items-center justify-center p-4";
    modal.innerHTML = `<div id="cluster-backdrop" class="absolute inset-0 bg-black/70"></div>
      <div class="relative bg-panel border border-edge rounded-2xl w-full max-w-xl max-h-[88vh] overflow-y-auto p-5 shadow-2xl">
        <div class="flex items-center justify-between mb-3">
          <h2 class="text-base font-semibold">🧩 <span id="cl-title"></span></h2>
          <button id="cluster-close" class="text-slate-400 hover:text-white text-xl leading-none">✕</button>
        </div>
        <div id="cluster-body"></div>
      </div>`;
    document.body.appendChild(modal);
    modal.querySelector("#cluster-backdrop")!.addEventListener("click", closeClusters);
    modal.querySelector("#cluster-close")!.addEventListener("click", closeClusters);
  }
  modal.classList.remove("hidden");
  (modal.querySelector("#cl-title") as HTMLElement).textContent = t("cl_title");
  const body = modal.querySelector("#cluster-body") as HTMLElement;
  if (!address) { // самостійний режим — поле вводу адреси + швидкий вибір відстежуваних
    body.innerHTML = `
      <input id="cl-input" placeholder="${t("cl_ph")}" class="w-full bg-panel2 border border-edge rounded-lg px-3 py-2 text-sm font-mono" />
      <div class="flex flex-wrap gap-1.5 mt-2">${state.wallets.map((w) => `<button data-cl-addr="${escAttr(w.address)}" class="text-[11px] px-2 py-1 rounded bg-panel2 border border-edge hover:border-accent">${w.label || shortAddr(w.address)}</button>`).join("")}</div>
      <button id="cl-go" class="w-full mt-3 bg-accent/15 hover:bg-accent/25 text-accent border border-accent/30 text-sm py-2 rounded-xl transition">${t("cl_run")}</button>`;
    const go = (): void => { const v = (body.querySelector("#cl-input") as HTMLInputElement).value.trim(); if (v) void openClusters(v); };
    body.querySelector("#cl-go")!.addEventListener("click", go);
    body.querySelector("#cl-input")!.addEventListener("keydown", (e) => { if ((e as KeyboardEvent).key === "Enter") go(); });
    body.querySelectorAll<HTMLElement>("[data-cl-addr]").forEach((b) => b.addEventListener("click", () => void openClusters(b.dataset.clAddr)));
    return;
  }
  body.innerHTML = `<div class="text-slate-400 text-sm py-8 text-center">${t("cl_analyzing")}</div>`;
  try {
    body.innerHTML = renderClusters(await getJSON<ClResp>("/api/wallet-clusters", { address }));
    const tw = body.querySelector("#cl-2hop") as HTMLElement | null;
    if (tw) tw.addEventListener("click", async () => {
      const out = body.querySelector("#cl-2hop-result") as HTMLElement;
      out.innerHTML = `<div class="text-[11px] text-slate-500 py-2">${t("cl_2hop_loading")}</div>`;
      tw.style.display = "none";
      try { out.innerHTML = render2hop(await getJSON<{ neighbors: number; clusters: Array<{ size: number; members: Array<{ addr: string; name: string | null; count: number }> }> }>("/api/wallet-2hop", { address: tw.dataset.addr })); }
      catch { out.innerHTML = `<div class="text-bad text-[11px] py-2">${t("conc_err")}</div>`; }
    });
  } catch { body.innerHTML = `<div class="text-bad text-sm py-8 text-center">${t("conc_err")}</div>`; }
}
export function closeClusters(): void { document.getElementById("cluster-modal")?.classList.add("hidden"); }

