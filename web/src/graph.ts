/* ---------- Граф зв'язків + Smart Money + кластери + деталі вузла ----------
   Когезивний модуль: усі спільно володіють станом графа (gNodes/gEdges/gView/…,
   ~14 приватних lets) і лоадерами. Крос-фічні виклики (openWalletAddress тощо)
   йдуть прямими імпортами (ці модулі не імпортують graph — циклів нема). */

import type { Wallet, BlockchainEvent, EntityType, Lang } from "./types";
import { t, lang } from "./i18n";
import { $, shortAddr, fmtUsd, fmtNum, fmtTime } from "./util";
import { state } from "./state";
import { escAttr, link, toast } from "./ui";
import { getJSON } from "./api";
import { BACKEND } from "./env";
import { priceOf } from "./prices";
import { networkById, explorerAddr } from "./networks";
import { arkhamLink, ENTITY_TYPE_CLS, ENTITY_HEX, WALLET_HEX } from "./attribution";
import { openWalletAddress, closePortfolio, getPortfolioWallet } from "./portfolio";
import { runConcentration, openConcentration, sparkline } from "./concentration";
import { nav } from "./nav";

/** Відкрити граф зв'язків, сфокусований на конкретному гаманці. */
export function openGraphForWallet(wid: string): void {
  // ВАЖЛИВО: спершу взяти поточний відкритий гаманець (вкл. зовнішні/нетрекнуті, яких нема в state.wallets),
  // бо closePortfolio() обнуляє portfolioWallet. Інакше граф показував би дефолтний гаманець зі списку.
  const pw = getPortfolioWallet();
  const w = (pw && pw.id === wid) ? pw : state.wallets.find((x) => x.id === wid);
  closePortfolio();
  if (!w) { openGraph(); return; }
  // зовнішній гаманець → зробити його доступним у списку графа (відображення + повторний вибір)
  if (!state.wallets.some((x) => x.id === w.id) && !gGraphExtWallets.some((x) => x.id === w.id)) gGraphExtWallets.push(w);
  gSelected = -1; gHover = -1; gDrag = null; gPan = null; gPlayersMode = false;
  gView = { scale: 1, ox: 0, oy: 0 };
  gNodes = []; gEdges = [];
  const modal = $("#graph-modal");
  modal.classList.remove("hidden"); modal.classList.add("flex");
  $("#graph-detail").classList.add("translate-x-full");
  populateGraphWalletSelect();
  graphHidden = false;
  cancelAnimationFrame(gRAF);
  gRAF = requestAnimationFrame(graphLoop);
  void loadWalletGraph(w);
}

/* ---------- Граф зв'язків (Arkham-style павутина) ---------- */

interface GNode {
  id: string; label: string; kind: "wallet" | "entity"; etype: EntityType | null;
  x: number; y: number; vx: number; vy: number; r: number; color: string; deg: number;
  verified?: boolean; // верифікована відома сутність (біржа/протокол)
}
interface GEdge { a: number; b: number; w: number; }


let gNodes: GNode[] = [];
let gEdges: GEdge[] = [];
let gRAF = 0;
let gView = { scale: 1, ox: 0, oy: 0 };
let gDrag: { idx: number } | null = null;
let gPan: { x: number; y: number } | null = null;
let gHover = -1;
let graphHidden = true;
let gRealMode = false; // true = граф із реальних даних блокчейну (ретро)
let gPlayersMode = false; // true = граф «гравці ↔ активи» (Smart Money)
let gGraphExtWallets: Wallet[] = []; // зовнішні (нетрекнуті) гаманці, відкриті в графі — щоб були в списку й повторно вибирались
let gLoadSeq = 0; // лічильник завантажень графа — ігнорувати застарілі відповіді
let gFocus: { type: "wallet" | "entity"; key: string } | null = null;
let gSelected = -1;
let gDownX = 0, gDownY = 0, gMoved = false, gDownIdx = -1, gPointerActive = false;

const nodeScreenR = (nd: GNode): number => nd.r * Math.min(1.4, Math.max(0.7, gView.scale));

const entityKeyOf = (ev: BlockchainEvent): string =>
  ev.counterpartyEntity.type === "unknown" ? `u:${ev.counterparty}` : `e:${ev.counterpartyEntity.name}`;

function buildGraphData(): void {
  const idxOf = new Map<string, number>();
  const nodes: GNode[] = [];
  const edgeMap = new Map<string, GEdge>();
  const nodeIndex = (key: string, make: () => GNode): number => {
    let i = idxOf.get(key);
    if (i === undefined) { i = nodes.length; idxOf.set(key, i); nodes.push(make()); }
    return i;
  };
  let evts = state.events;
  if (gFocus?.type === "wallet") evts = evts.filter((e) => e.walletId === gFocus!.key);
  else if (gFocus?.type === "entity") evts = evts.filter((e) => entityKeyOf(e) === gFocus!.key);
  for (const ev of evts.slice(0, gFocus ? 200 : 70)) {
    const wallet = state.wallets.find((w) => w.id === ev.walletId);
    if (!wallet) continue;
    const ai = nodeIndex(`w:${wallet.id}`, () => ({
      id: `w:${wallet.id}`, label: wallet.label || shortAddr(wallet.address), kind: "wallet", etype: null,
      x: 0, y: 0, vx: 0, vy: 0, r: 0, color: WALLET_HEX, deg: 0,
    }));
    const ent = ev.counterpartyEntity;
    const ekey = ent.type === "unknown" ? `u:${ev.counterparty}` : `e:${ent.name}`;
    const bi = nodeIndex(ekey, () => ({
      id: ekey, label: ent.type === "unknown" ? shortAddr(ev.counterparty) : ent.name, kind: "entity", etype: ent.type,
      x: 0, y: 0, vx: 0, vy: 0, r: 0, color: ENTITY_HEX[ent.type], deg: 0,
    }));
    const lo = Math.min(ai, bi), hi = Math.max(ai, bi);
    const k = `${lo}-${hi}`;
    const e = edgeMap.get(k);
    if (e) e.w += 1; else edgeMap.set(k, { a: lo, b: hi, w: 1 });
    nodes[ai].deg += 1; nodes[bi].deg += 1;
  }
  const n = nodes.length;
  nodes.forEach((nd, i) => {
    const ang = (i / Math.max(1, n)) * Math.PI * 2;
    const rad = 70 + (i % 5) * 28;
    nd.x = Math.cos(ang) * rad;
    nd.y = Math.sin(ang) * rad;
    nd.r = (nd.kind === "wallet" ? 11 : 6) + Math.min(14, Math.sqrt(nd.deg) * 3);
  });
  gNodes = nodes;
  gEdges = [...edgeMap.values()];
}

function stepPhysics(): void {
  const n = gNodes.length;
  if (n === 0) return;
  const REP = 2400, SPRING = 0.03, LEN = 95, GRAV = 0.02, DAMP = 0.85, MAX = 30;
  const fx = new Float64Array(n), fy = new Float64Array(n);
  for (let i = 0; i < n; i++) {
    const a = gNodes[i];
    for (let j = i + 1; j < n; j++) {
      const b = gNodes[j];
      let dx = a.x - b.x, dy = a.y - b.y;
      let d2 = dx * dx + dy * dy;
      if (d2 < 0.01) { dx = (i - j) || 1; dy = 1; d2 = dx * dx + dy * dy; }
      const d = Math.sqrt(d2);
      const f = REP / d2, ux = dx / d, uy = dy / d;
      fx[i] += ux * f; fy[i] += uy * f;
      fx[j] -= ux * f; fy[j] -= uy * f;
    }
    fx[i] -= a.x * GRAV; fy[i] -= a.y * GRAV;
  }
  for (const e of gEdges) {
    const a = gNodes[e.a], b = gNodes[e.b];
    const dx = b.x - a.x, dy = b.y - a.y;
    const d = Math.sqrt(dx * dx + dy * dy) || 1;
    const f = SPRING * (d - LEN), ux = dx / d, uy = dy / d;
    fx[e.a] += ux * f; fy[e.a] += uy * f;
    fx[e.b] -= ux * f; fy[e.b] -= uy * f;
  }
  for (let i = 0; i < n; i++) {
    const nd = gNodes[i];
    if (gDrag && gDrag.idx === i) { nd.vx = 0; nd.vy = 0; continue; }
    nd.vx = Math.max(-MAX, Math.min(MAX, (nd.vx + fx[i]) * DAMP));
    nd.vy = Math.max(-MAX, Math.min(MAX, (nd.vy + fy[i]) * DAMP));
    nd.x += nd.vx; nd.y += nd.vy;
  }
}

function drawGraph(): void {
  const canvas = $<HTMLCanvasElement>("#graph-canvas");
  const ctx = canvas.getContext("2d");
  if (!ctx) return;
  const rect = canvas.getBoundingClientRect();
  const dpr = window.devicePixelRatio || 1;
  const W = Math.round(rect.width * dpr), H = Math.round(rect.height * dpr);
  if (canvas.width !== W || canvas.height !== H) { canvas.width = W; canvas.height = H; }
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.clearRect(0, 0, rect.width, rect.height);
  const cx = rect.width / 2 + gView.ox, cy = rect.height / 2 + gView.oy, s = gView.scale;
  const sx = (x: number): number => cx + x * s, sy = (y: number): number => cy + y * s;

  for (const e of gEdges) {
    const a = gNodes[e.a], b = gNodes[e.b];
    const hot = gHover === e.a || gHover === e.b || gSelected === e.a || gSelected === e.b;
    ctx.strokeStyle = hot ? "rgba(59,130,246,0.75)" : "rgba(100,116,139,0.22)";
    ctx.lineWidth = hot ? 2 : Math.min(4, 0.6 + e.w * 0.5);
    ctx.beginPath(); ctx.moveTo(sx(a.x), sy(a.y)); ctx.lineTo(sx(b.x), sy(b.y)); ctx.stroke();
  }

  ctx.font = "11px ui-sans-serif, system-ui, sans-serif";
  ctx.textAlign = "center";
  gNodes.forEach((nd, i) => {
    const active = i === gHover || i === gSelected;
    const x = sx(nd.x), y = sy(nd.y), r = nodeScreenR(nd);
    ctx.beginPath(); ctx.arc(x, y, r, 0, Math.PI * 2);
    ctx.fillStyle = nd.color; ctx.globalAlpha = active ? 1 : 0.92; ctx.fill(); ctx.globalAlpha = 1;
    ctx.lineWidth = i === gSelected ? 4 : active ? 3 : 1.5;
    ctx.strokeStyle = i === gSelected ? "#ffffff" : nd.kind === "wallet" ? "#dbeafe" : "rgba(255,255,255,0.5)";
    ctx.stroke();
    ctx.fillStyle = "#cbd5e1";
    ctx.fillText(nd.label, x, y + r + 12);
  });
}

function graphLoop(): void {
  stepPhysics(); stepPhysics();
  drawGraph();
  gRAF = requestAnimationFrame(graphLoop);
}

function graphPick(mx: number, my: number): number {
  const rect = $<HTMLCanvasElement>("#graph-canvas").getBoundingClientRect();
  const cxx = rect.width / 2 + gView.ox, cyy = rect.height / 2 + gView.oy, s = gView.scale;
  for (let i = gNodes.length - 1; i >= 0; i--) {
    const nd = gNodes[i];
    const x = cxx + nd.x * s, y = cyy + nd.y * s, r = nodeScreenR(nd) + 4;
    if ((mx - x) ** 2 + (my - y) ** 2 <= r * r) return i;
  }
  return -1;
}

export function renderGraphChrome(): void {
  const w = gNodes.filter((n) => n.kind === "wallet").length;
  const ent = gNodes.filter((n) => n.kind === "entity").length;
  $("#graph-stats").innerHTML =
    `<span style="color:${WALLET_HEX}">●</span> ${w} ${t("wallets")} &nbsp; ` +
    `<span style="color:#a855f7">●</span> ${ent} ${t("graph_entities")} &nbsp; ` +
    `<span class="text-slate-400">↔</span> ${gEdges.length} ${t("graph_links")}`;
  const types: EntityType[] = ["cex", "fund", "defi", "issuer", "individual", "unknown"];
  $("#graph-legend").innerHTML =
    `<div class="flex items-center gap-2"><span class="w-2.5 h-2.5 rounded-full inline-block" style="background:${WALLET_HEX}"></span>${t("wallets")}</div>` +
    types.map((ty) => `<div class="flex items-center gap-2"><span class="w-2.5 h-2.5 rounded-full inline-block" style="background:${ENTITY_HEX[ty]}"></span>${t(`entity_${ty}`)}</div>`).join("");
}

/* ---------- Smart Money: whale-активи + крос-актив гравці ---------- */
type SmAsset = { rank: number; symbol: string; contract: string; top10pct: number; band: string; sector: string };
let smAssets: SmAsset[] = [];
let smGroupBy: "sector" | "conc" = "sector";
const smSecFilter = new Set<string>();
const smConcFilter = new Set<string>();
const SECTOR_ORDER = ["depin", "memecoin", "defi", "rwa", "other"];
const SM_CONC_TIERS: Array<[string, string, (p: number) => boolean]> = [
  ["hi", "\U0001F534 70%+", (p) => p > 70],
  ["mid", "\U0001F7E0 40–70%", (p) => p >= 40 && p <= 70],
  ["lo", "\U0001F7E1 15–40%", (p) => p >= 15 && p < 40],
];
function smAssetBtn(a: SmAsset): string {
  return `<button data-sm-asset="${escAttr(a.contract)}" class="flex items-center justify-between gap-2 text-left bg-panel2 border border-edge hover:border-accent rounded-lg px-2.5 py-1.5 text-xs transition">
      <span><span class="font-medium">${a.symbol}</span> <span class="text-slate-500">#${a.rank}</span></span>
      <span class="px-1.5 py-0.5 rounded ${a.top10pct > 70 ? "bg-bad/20 text-bad" : a.top10pct >= 40 ? "bg-amber-500/20 text-amber-400" : "bg-accent/20 text-accent"} text-[11px]">${a.top10pct}%</span></button>`;
}
function renderAssetsSection(): string {
  const secOn = (s: string) => smSecFilter.size === 0 || smSecFilter.has(s);
  const concOn = (p: number) => smConcFilter.size === 0 || SM_CONC_TIERS.some(([k, , pred]) => smConcFilter.has(k) && pred(p));
  const filtered = smAssets.filter((a) => secOn(a.sector) && concOn(a.top10pct));
  const secCount = (s: string) => smAssets.filter((a) => a.sector === s).length;
  const gbtn = (key: "sector" | "conc", label: string) =>
    `<button data-sm-group="${key}" class="px-2 py-0.5 rounded text-[11px] ${smGroupBy === key ? "bg-accent/25 text-accent border border-accent/40" : "bg-panel2 text-slate-400 border border-edge"}">${label}</button>`;
  const chip = (attr: string, val: string, label: string, active: boolean, n?: number) =>
    `<button data-${attr}="${escAttr(val)}" class="px-1.5 py-0.5 rounded text-[11px] ${active ? "bg-accent/20 text-accent border border-accent/40" : "bg-panel2 text-slate-500 border border-edge"}">${label}${n !== undefined ? ` <span class="opacity-60">${n}</span>` : ""}</button>`;
  const secChips = SECTOR_ORDER.filter((s) => secCount(s) > 0).map((s) => chip("sm-secfilter", s, t("sec_" + s), smSecFilter.has(s), secCount(s))).join("");
  const allChip = chip("sm-secfilter", "__all", t("sm_filter_all"), smSecFilter.size === 0);
  const concChips = SM_CONC_TIERS.map(([k, lbl]) => chip("sm-concfilter", k, lbl, smConcFilter.has(k))).join("");
  const bar = `<div class="flex flex-wrap items-center gap-1.5 mb-2"><span class="text-[11px] text-slate-500">${t("sm_group_by")}</span>${gbtn("sector", t("sm_group_sector"))}${gbtn("conc", t("sm_group_conc"))}</div>
    <div class="flex flex-wrap items-center gap-1 mb-1">${allChip}${secChips}</div>
    <div class="flex flex-wrap items-center gap-1 mb-2">${concChips}</div>`;
  const head = `<div class="text-xs text-slate-400 mb-1">${t("sm_assets_h")} <span class="text-slate-500">· ${filtered.length}</span></div>`;
  if (!filtered.length) return head + bar + `<div class="text-[11px] text-slate-500 py-3">—</div>`;
  let groups: Array<[string, SmAsset[]]>;
  if (smGroupBy === "sector") {
    groups = SECTOR_ORDER.map((s) => [t("sec_" + s), filtered.filter((a) => a.sector === s).sort((x, y) => y.top10pct - x.top10pct)] as [string, SmAsset[]]).filter(([, l]) => l.length);
  } else {
    groups = SM_CONC_TIERS.map(([, lbl, pred]) => [lbl + " " + t("sm_tier"), filtered.filter((a) => pred(a.top10pct)).sort((x, y) => y.top10pct - x.top10pct)] as [string, SmAsset[]]).filter(([, l]) => l.length);
  }
  const list = groups.map(([label, arr]) => `<div class="text-[11px] text-slate-500 mb-1 mt-2 font-medium">${label} · ${arr.length}</div><div class="grid grid-cols-2 gap-1.5">${arr.map(smAssetBtn).join("")}</div>`).join("");
  return head + bar + list + `<div class="mb-4"></div>`;
}
function rerenderAssets(): void { const el = document.getElementById("sm-assets"); if (el) el.innerHTML = renderAssetsSection(); }

interface SignalsData { familiesCount: number; familySignals: Array<{ root: string; size: number; volumeUsd: number; phase: string; sector?: string; topEmission?: Array<{ symbol: string; emissionPct: number }> }> }

// Actionable digest at the top of Smart Money: which whale families are
// actively distributing (selling out) vs accumulating, at a glance.
function renderSignals(sig: SignalsData | null): string {
  if (!sig || !Array.isArray(sig.familySignals) || !sig.familySignals.length) return "";
  const chip = (x: SignalsData["familySignals"][number]) => {
    const cls = x.phase === "distributing" ? "border-bad/40 text-bad" : "border-good/40 text-good";
    const arrow = x.phase === "distributing" ? "\ud83d\udcc9" : "\ud83d\udcc8";
    const sec = x.sector ? ` \u00b7 ${NARR_EMO[x.sector] || ""} ${t("sec_" + x.sector)}` : "";
    const em = x.topEmission && x.topEmission[0] ? ` \u00b7 ${x.topEmission[0].symbol} ${x.topEmission[0].emissionPct}%` : "";
    return `<button data-open-families class="text-left text-[10px] px-2 py-1 rounded border ${cls} bg-panel hover:opacity-80">${arrow} <b>${x.size}</b> ${t("fam_wallets")}${sec}${em}</button>`;
  };
  const dist = sig.familySignals.filter((x) => x.phase === "distributing");
  const acc = sig.familySignals.filter((x) => x.phase === "accumulating");
  const row = (label: string, cls: string, items: SignalsData["familySignals"]) => items.length ? `<div class="mb-1 flex flex-wrap gap-1 items-center"><span class="text-[10px] ${cls} mr-1">${label} (${items.length}):</span>${items.slice(0, 8).map(chip).join("")}</div>` : "";
  return `<div class="mb-3 bg-panel2 border border-edge rounded-lg p-2.5"><div class="text-xs text-slate-400 mb-1.5">\ud83d\udea8 ${t("sig_title")}</div>${row(t("sig_distrib"), "text-bad", dist)}${row(t("sig_accum"), "text-good", acc)}</div>`;
}

interface FamMember { address: string; volumeUsd: number; degree: number }
interface FamData { familiesCount: number; groupedWallets: number; families: Array<{ size: number; volumeUsd: number; edges: number; root: string; transfers?: number; sectorMix?: Array<{ sector: string; pct: number }>; topEmission?: Array<{ symbol: string; emissionPct: number }>; phase?: string; dominantSector?: string; members: FamMember[] }> }

function renderFamilies(d: FamData): string {
  const fams = (d.families || []).slice(0, 40);
  if (!fams.length) return `<div class="text-slate-500 text-sm py-6 text-center">\u2014</div>`;
  const head = `<div class="text-[11px] text-slate-500 mb-2">${d.familiesCount} ${t("fam_families")} \u00b7 ${d.groupedWallets} ${t("fam_wallets")} \u00b7 <span class="text-slate-400">${t("fam_note")}</span></div>`;
  const cards = fams.map((f, i) => {
    const vol = f.volumeUsd >= 1e6 ? `$${(f.volumeUsd / 1e6).toFixed(1)}M` : `$${(f.volumeUsd / 1e3).toFixed(0)}K`;
    const sec = (f.sectorMix || []).filter((s) => s.sector !== "other" && s.pct > 0).map((s) => `<span class="text-[10px] text-slate-400">${NARR_EMO[s.sector] || ""} ${s.pct}%</span>`).join(" ");
    const em = (f.topEmission || []).slice(0, 6).map((e) => `<span class="text-[10px] px-1.5 py-0.5 rounded bg-accent/15 text-accent">${e.symbol} ${e.emissionPct}%</span>`).join(" ");
    const mem = (f.members || []).slice(0, 12).map((m) => `<span data-wallet-addr="${escAttr(m.address)}" class="text-[10px] font-mono px-1.5 py-0.5 rounded bg-panel border border-edge text-slate-400 cursor-pointer hover:text-accent">${shortAddr(m.address)}</span>`).join("");
    const more = f.size > 12 ? `<span class="text-[10px] text-slate-500">+${f.size - 12}</span>` : "";
    const dom = f.dominantSector ? `<span class="text-[10px] px-1.5 py-0.5 rounded bg-panel border border-edge">${NARR_EMO[f.dominantSector] || ""} ${t("sec_" + f.dominantSector)}</span>` : "";
    const ph = f.phase === "accumulating" ? `<span class="text-[10px] px-1.5 py-0.5 rounded bg-panel border border-edge text-good">${t("fam_accum")}</span>` : f.phase === "distributing" ? `<span class="text-[10px] px-1.5 py-0.5 rounded bg-panel border border-edge text-bad">${t("fam_distrib")}</span>` : "";
    return `<div class="rounded-lg border border-fuchsia-500/25 bg-fuchsia-500/5 p-3 mb-2">
      <div class="flex justify-between items-center mb-1"><span class="text-sm font-semibold text-fuchsia-200">${t("fam_family")} #${i + 1} \u00b7 ${f.size} ${t("fam_wallets")}</span><span class="text-accent text-sm font-semibold">${vol}</span></div>
      <div class="text-[10px] text-slate-500 mb-1">${f.transfers ? `${f.transfers.toLocaleString()} tx \u00b7 ${sec || "\u2014"}` : `${f.edges} ${t("fam_links")}`}</div>
      ${(dom || ph) ? `<div class="flex flex-wrap gap-1 mb-1.5">${ph}${dom}</div>` : ""}
      ${em ? `<div class="mb-1.5 text-[10px]"><span class="text-slate-400">${t("fam_emission")}:</span> ${em}</div>` : ""}
      <div class="flex flex-wrap gap-1">${mem}${more}</div>
    </div>`;
  }).join("");
  return head + cards;
}

export async function openFamilies(): Promise<void> {
  let modal = document.getElementById("fam-modal");
  if (!modal) {
    modal = document.createElement("div");
    modal.id = "fam-modal";
    modal.className = "fixed inset-0 z-50 flex items-center justify-center p-4";
    modal.innerHTML = `<div id="fam-backdrop" class="absolute inset-0 bg-black/70"></div>
      <div class="relative bg-panel border border-edge rounded-2xl w-full max-w-2xl max-h-[88vh] overflow-y-auto p-5 shadow-2xl">
        <div class="flex items-center justify-between mb-3"><h2 class="text-base font-semibold">\ud83d\udd78 ${t("fam_title")}</h2>
        <button id="fam-close" class="text-slate-400 hover:text-white text-xl leading-none">\u2715</button></div>
        <div class="mb-3"><div class="flex gap-2"><input id="ask-input" placeholder="${t('ask_ph')}" class="flex-1 bg-panel2 border border-edge rounded px-2 py-1.5 text-sm"><button id="ask-btn" class="px-3 py-1.5 rounded bg-accent/20 text-accent border border-accent/40 text-sm shrink-0">\ud83e\udd16 ${t('ask_btn')}</button></div><div id="ask-answer" class="text-sm text-slate-300 mt-2 whitespace-pre-wrap"></div></div>
        <div id="fam-body"></div>
      </div>`;
    document.body.appendChild(modal);
    const hide = () => modal!.classList.add("hidden");
    modal.querySelector("#fam-backdrop")!.addEventListener("click", hide);
    modal.querySelector("#fam-close")!.addEventListener("click", hide);
    modal.querySelector("#fam-body")!.addEventListener("click", (e) => {
      if ((e.target as HTMLElement).closest("[data-wallet-addr]")) hide();
    });
    const runAsk = async () => {
      const inp = modal!.querySelector<HTMLInputElement>("#ask-input")!;
      const out = modal!.querySelector<HTMLElement>("#ask-answer")!;
      const qq = inp.value.trim();
      if (!qq) return;
      out.textContent = t("ask_thinking");
      try {
        const d = await getJSON<{ answer?: string; error?: string }>("/api/ask", { q: qq });
        out.textContent = d.answer || d.error || "\u2014";
      } catch (e) { out.textContent = (e as Error).message; }
    };
    modal.querySelector("#ask-btn")!.addEventListener("click", () => void runAsk());
    modal.querySelector("#ask-input")!.addEventListener("keydown", (e) => { if ((e as KeyboardEvent).key === "Enter") void runAsk(); });
  }
  modal.classList.remove("hidden");
  const body = modal.querySelector("#fam-body") as HTMLElement;
  body.innerHTML = `<div class="text-slate-400 text-sm py-8 text-center">${t("sm_loading")}</div>`;
  try {
    const d = await getJSON<FamData>("/api/families");
    body.innerHTML = renderFamilies(d);
  } catch (e) {
    body.innerHTML = `<div class="text-sm text-bad">${(e as Error).message}</div>`;
  }
}

interface Rotation { months: string[]; sectors: Record<string, { net: number[] }> }

// Temporal narrative rotation: monthly NET whale flow per narrative (from 8y of
// backfill). Multi-line, zero-centred — accumulation above 0, distribution below.
function renderRotation(rot: Rotation | null): string {
  if (!rot || !Array.isArray(rot.months) || rot.months.length < 3) return "";
  const N = Math.min(24, rot.months.length);
  const months = rot.months.slice(-N);
  const secs: Array<[string, string]> = [["depin", "#22d3ee"], ["memecoin", "#f472b6"], ["defi", "#34d399"], ["rwa", "#a78bfa"]];
  const series = secs.map(([s, c]) => ({ s, c, v: (rot.sectors[s]?.net || []).slice(-N) }));
  const max = Math.max(1, ...series.flatMap((x) => x.v.map((n) => Math.abs(n))));
  const W = 280, H = 88, pad = 4;
  const px = (i: number) => pad + (i / Math.max(1, N - 1)) * (W - 2 * pad);
  const py = (v: number) => H / 2 - (v / max) * (H / 2 - pad);
  const zero = `<line x1="${pad}" y1="${H / 2}" x2="${W - pad}" y2="${H / 2}" stroke="#334155" stroke-width="0.5"/>`;
  const lines = series.map((ser) => {
    if (!ser.v.length) return "";
    const d = ser.v.map((v, i) => `${i ? "L" : "M"}${px(i).toFixed(1)},${py(v).toFixed(1)}`).join(" ");
    return `<path d="${d}" fill="none" stroke="${ser.c}" stroke-width="1.5"/>`;
  }).join("");
  const legend = series.map((ser) => `<span class="text-[9px] whitespace-nowrap" style="color:${ser.c}">\u25cf ${t("sec_" + ser.s)}</span>`).join(" ");
  return `<div class="bg-panel2 border border-edge rounded-lg p-2 mb-3">
    <div class="flex justify-between text-[10px] text-slate-400 mb-1"><span>${t("rot_title")}</span><span>${months[0]} \u2192 ${months[months.length - 1]}</span></div>
    <svg viewBox="0 0 ${W} ${H}" class="w-full" preserveAspectRatio="none">${zero}${lines}</svg>
    <div class="flex flex-wrap gap-2 mt-1">${legend}</div>
  </div>`;
}

interface SectorFlow { sectors: Array<{ sector: string; assets: number; avgTop10: number; highConc: number; recentEntries: number; topAssets: string[] }> }

const NARR_EMO: Record<string, string> = { depin: "\ud83d\udce1", memecoin: "\ud83d\udc38", defi: "\ud83c\udfe6", rwa: "\ud83c\udfdb", other: "\u25fd" };

// "Narratives" overview: where smart money sits and where it is flowing NOW.
// Concentration (avg top-10) + momentum (fresh whale entries) per narrative.
function renderNarratives(sf: SectorFlow | null): string {
  if (!sf || !Array.isArray(sf.sectors)) return "";
  const narr = sf.sectors.filter((x) => x.sector !== "other" && x.assets > 0);
  if (!narr.length) return "";
  const maxMom = Math.max(...narr.map((x) => x.recentEntries), 1);
  const cards = narr.map((x) => {
    const hot = x.recentEntries === maxMom && x.recentEntries > 0;
    return `<button data-narr-sector="${x.sector}" class="text-left bg-panel2 border ${hot ? "border-accent/50" : "border-edge"} hover:border-accent rounded-lg p-2 transition">
      <div class="flex items-center justify-between mb-0.5"><span class="text-xs font-semibold">${NARR_EMO[x.sector] || ""} ${t("sec_" + x.sector)}</span>${hot ? `<span class="text-[9px] px-1 rounded bg-accent/20 text-accent">${t("narr_hot")}</span>` : ""}</div>
      <div class="text-[10px] text-slate-400">${x.assets} ${t("narr_assets")} \u00b7 ${t("narr_conc")} ${x.avgTop10}%</div>
      <div class="text-[10px] mt-0.5">${x.recentEntries > 0 ? `<span class="text-good">\u2197 ${x.recentEntries} ${t("narr_entries")}</span>` : `<span class="text-slate-500">\u2014 ${t("narr_quiet")}</span>`}</div>
      <div class="text-[9px] text-slate-500 truncate mt-0.5">${x.topAssets.slice(0, 4).join(" \u00b7 ")}</div>
    </button>`;
  }).join("");
  return `<div class="mb-3"><div class="flex items-center justify-between mb-1.5"><span class="text-xs text-slate-400">\ud83e\udded ${t("narr_title")}</span><button data-open-families class="text-[10px] px-2 py-0.5 rounded bg-fuchsia-500/15 text-fuchsia-300 border border-fuchsia-500/30 hover:bg-fuchsia-500/25">\ud83d\udd78 ${t("fam_open")}</button></div><div class="grid grid-cols-2 gap-1.5">${cards}</div></div>`;
}

function renderSmartMoney(wa: { assets: Array<{ rank: number; symbol: string; contract: string; top10pct: number; band: string; sector: string }> }, ca: { multiAsset?: Array<{ address: string; count: number; totalSharePct: number; label?: string; assets: Array<{ symbol: string; share: number }> }> }, af?: { assets?: Array<{ symbol: string; rank: number; groups: Array<{ combinedShare: number; members: Array<{ addr: string; share: number }>; commonFunders: Array<{ funder: string; avgUsd: number }> }> }> }): string {
  const assets = (wa.assets || []).filter((a) => a.top10pct >= 15).sort((a, b) => b.top10pct - a.top10pct);
  if (!assets.length) return `<div class="text-slate-500 text-sm py-6 text-center">${t("sm_empty")}</div>`;
  smAssets = assets;
  const aSec = `<div id="sm-assets">${renderAssetsSection()}</div>`;
  const players = ca.multiAsset || [];
  const pSec = `<div class="flex items-center justify-between mb-1.5">
      <span class="text-xs text-slate-400">${t("sm_players_h")}</span>
      <button data-sm-graph="1" class="text-[11px] px-2 py-1 rounded bg-accent/15 text-accent border border-accent/30 hover:bg-accent/25 transition">🕸 ${t("sm_graph_btn")}</button></div>
    ${players.slice(0, 20).map((p, i) => `<button data-wallet-addr="${escAttr(p.address)}" class="w-full text-left bg-panel2 border border-edge hover:border-accent rounded-lg px-3 py-2 mb-1.5 transition">
      <div class="flex justify-between text-xs"><span class="font-mono"><span class="text-slate-500 mr-1">${i + 1}.</span>${p.label || shortAddr(p.address)}</span><span class="text-accent">${p.count} ${t("sm_in_assets")}</span></div>
      <div class="text-[11px] text-slate-500 truncate">${p.assets.map((x) => `${x.symbol} ${x.share}%`).join(" · ")}</div></button>`).join("") || `<div class="text-[11px] text-slate-500">—</div>`}`;
  const afa = af?.assets || [];
  const fSec = afa.length ? `<div class="text-xs text-slate-400 mb-1 mt-4">${t("sm_forensics_h")}</div>` +
    afa.slice(0, 25).map((a) => `<div class="rounded-lg border border-edge bg-panel2 p-2 mb-1.5">
      <div class="flex justify-between text-xs mb-1"><span class="font-medium">${a.symbol} <span class="text-slate-500">#${a.rank}</span></span><span class="text-bad">${a.groups.length} ${t("sm_coord_groups")}</span></div>
      ${a.groups.slice(0, 2).map((g) => `<div class="text-[11px] text-slate-400 mt-0.5">🏛 <b>${g.combinedShare}%</b> ${t("ag_of_supply")} · ${g.members.length} ${t("cg_members")}${g.commonFunders.length ? ` · ${g.commonFunders.length} ${t("cg_common")}` : ""}</div>
        <div class="truncate">${g.members.slice(0, 5).map((m) => `<span data-wallet-addr="${escAttr(m.addr)}" class="text-[11px] font-mono text-slate-500 cursor-pointer hover:text-accent mr-1.5">${shortAddr(m.addr)}·${m.share}%</span>`).join("")}</div>`).join("")}
    </div>`).join("") : "";
  return `<div id="sm-liquidations"></div>` + `<div id="sm-backtest"></div>` + `<div id="sm-moves"></div>` + `<div id="sm-networks"></div>` + aSec + `<div id="sm-positions"></div>` + fSec + pSec;
}

interface BtSignal {
  samples: number; wallets: number; hitRate: number; hitRateCi95: [number, number] | null;
  meanExcess: number | null; medianExcess: number | null; meanRaw: number | null; meanBench: number | null;
  pendingEvents: number; assetsScored: number;
}
interface BtMethod { horizonDays: number; benchmark: string; whaleUsd: number; caveats: string[]; }
interface Backtest {
  generated: number; status: { running: boolean; processed: number; total: number };
  method: BtMethod | null; signal: BtSignal | null;
  assets: Array<{ symbol: string; rank: number; events: number; resolved: number; meanExcess: number | null }>;
}

// Подієвий бектест: вхід кита → фіксований горизонт → надлишкова дохідність над ETH.
interface CoordNet { size: number; sharedAssets: Array<{ symbol: string; contract: string | null; marketCap: number | null }>; assetCount: number; conductor: string | null; members: Array<{ addr: string; assetCount: number }>; collective: Array<{ symbol: string; combinedShare: number; holders: number; contract: string | null; marketCap: number | null }>; }
let smNetworks: CoordNet[] = [];
let cgNetworks: CoordNet[] = []; // мережі у вкладці «Групи капіталу» (для обробників розкриття/графа)
// ЛІКВІДАЦІЇ ФʼЮЧЕРСІВ: агрегований фід примусових ліквідацій Binance (лонги/шорти + топ, вікна 5хв/1год/24год).
interface LiqAgg { long: number; short: number; total: number; count: number; top: Array<{ symbol: string; usd: number }> }
interface LiqData { connected: boolean; events: number; w5m: LiqAgg; w1h: LiqAgg; w24h: LiqAgg; flushThreshold: number; }
async function loadLiquidations(): Promise<void> {
  const el = document.getElementById("sm-liquidations");
  if (!el) return;
  try {
    const d = await getJSON<LiqData>("/api/liquidations");
    if (!d || (!d.connected && !d.events)) { el.innerHTML = `<div class="rounded-lg border border-edge bg-panel2 p-2 mb-2 text-[11px] text-slate-500">💥 ${t("liq_h")}: ${t("liq_off")}</div>`; return; }
    const bar = (a: LiqAgg): string => {
      const tot = a.total || 1, lp = Math.round((a.long / tot) * 100);
      return `<div class="flex h-2 rounded-full overflow-hidden bg-panel"><div style="width:${lp}%" class="bg-good/70"></div><div style="width:${100 - lp}%" class="bg-bad/70"></div></div>`;
    };
    const win = (label: string, a: LiqAgg): string => `<div class="mb-1.5">
      <div class="flex justify-between text-[11px] mb-0.5"><span class="text-slate-400">${label}</span><span class="text-slate-300"><b>${fmtUsd(a.total)}</b> <span class="text-slate-500">(${a.count})</span></span></div>
      ${bar(a)}
      <div class="flex justify-between text-[10px] mt-0.5"><span class="text-good">${t("liq_long")} ${fmtUsd(a.long)}</span><span class="text-bad">${t("liq_short")} ${fmtUsd(a.short)}</span></div></div>`;
    const top = d.w1h.top?.length ? d.w1h.top : d.w24h.top || [];
    el.innerHTML = `<div class="rounded-lg border border-fuchsia-500/25 bg-fuchsia-500/5 p-2.5 mb-2">
      <div class="flex justify-between items-center mb-1.5"><span class="text-xs font-semibold text-fuchsia-300">💥 ${t("liq_h")}</span><span class="text-[10px] ${d.connected ? "text-good" : "text-slate-500"}">${d.connected ? "● live" : "○"}</span></div>
      ${win(t("liq_5m"), d.w5m)}${win(t("liq_1h"), d.w1h)}${win(t("liq_24h"), d.w24h)}
      ${top.length ? `<div class="text-[10px] text-slate-400 mt-1 mb-0.5">${t("liq_top")}:</div>
      <div class="flex flex-wrap gap-1">${top.slice(0, 6).map((x) => `<span class="text-[10px] px-1.5 py-0.5 rounded bg-panel text-slate-300">${x.symbol} ${fmtUsd(x.usd)}</span>`).join("")}</div>` : ""}</div>`;
  } catch { el.innerHTML = ""; }
}

// СИНХРОННІ РУХИ: коли ≥3 члени мережі рухнули один актив у вузькому вікні — найсильніший сигнал змови.
async function loadCoordinatedMoves(): Promise<void> {
  const el = document.getElementById("sm-moves");
  if (!el) return;
  try {
    const d = await getJSON<{ moves: Array<{ asset: string; direction: string; count: number; totalMembers: number; members: string[]; latestTs: number; conductor: string | null; marketCap: number | null }> }>("/api/coordinated-moves");
    const m = d.moves || [];
    if (!m.length) { el.innerHTML = ""; return; }
    el.innerHTML = `<div class="text-xs text-orange-300 mb-1 font-semibold">🔥 ${t("mv_h")} <span class="text-slate-500 font-normal">${m.length}</span></div>` +
      m.slice(0, 10).map((x) => { const acc = x.direction === "accumulate"; return `<div class="rounded-lg border ${acc ? "border-good/30 bg-good/5" : "border-bad/30 bg-bad/5"} p-2 mb-1.5">
        <div class="flex justify-between items-center"><span class="text-xs font-semibold ${acc ? "text-good" : "text-bad"}">${acc ? "🟢 " + t("mv_accum") : "🔴 " + t("mv_dist")} <b>${x.asset}</b>${x.marketCap ? ` <span class="text-slate-500 font-normal">${fmtUsd(x.marketCap)}</span>` : ""}</span><span class="text-[10px] text-slate-400">${x.count}/${x.totalMembers} · ${fmtTime(x.latestTs)}</span></div>
        <div class="flex flex-wrap gap-1 mt-1">${x.members.slice(0, 8).map((a) => `<span data-wallet-addr="${escAttr(a)}" class="text-[10px] font-mono px-1.5 py-0.5 rounded bg-panel border border-edge text-slate-400 cursor-pointer hover:text-accent">${shortAddr(a)}</span>`).join("")}</div>
      </div>`; }).join("");
  } catch { el.innerHTML = ""; }
}

// КООРДИНОВАНІ МЕРЕЖІ: кластери китів, що сидять разом у тих самих активах (+ диригент).
async function loadCoordinatedNetworks(): Promise<void> {
  const el = document.getElementById("sm-networks");
  if (!el) return;
  try {
    const d = await getJSON<{ count: number; networks: CoordNet[] }>("/api/coordinated-networks");
    const nets = d.networks || [];
    smNetworks = nets;
    if (!nets.length) { el.innerHTML = ""; return; }
    el.innerHTML = `<div class="text-xs text-fuchsia-300 mb-1 font-semibold">🕸 ${t("net_h")} <span class="text-slate-500 font-normal">${nets.length}</span></div>` +
      nets.slice(0, 15).map((n, i) => `<div class="rounded-lg border border-fuchsia-500/30 bg-fuchsia-500/5 p-2 mb-1.5">
        <div class="flex justify-between items-center mb-0.5"><span class="text-xs font-semibold text-fuchsia-300">${t("net_one")} ${i + 1} · ${n.size} ${t("cg_members")}</span>${n.conductor ? `<span data-wallet-addr="${escAttr(n.conductor)}" class="text-[10px] text-amber-400 cursor-pointer hover:underline">🎼 ${shortAddr(n.conductor)}</span>` : ""}</div>
        ${n.sharedAssets.length ? `<div class="text-[11px] text-slate-400 mb-1">${t("net_shared")}: <span class="text-slate-300">${n.sharedAssets.slice(0, 10).map((a) => a.marketCap ? `${a.symbol} (${fmtUsd(a.marketCap)})` : a.symbol).join(", ")}</span></div>` : ""}
        <div class="flex flex-wrap gap-1 mb-1.5">${n.members.slice(0, 8).map((m) => `<span data-wallet-addr="${escAttr(m.addr)}" class="text-[10px] font-mono px-1.5 py-0.5 rounded bg-panel border border-edge text-slate-400 cursor-pointer hover:text-accent">${shortAddr(m.addr)}·${m.assetCount}</span>`).join("")}</div>
        <div class="flex gap-3"><button data-net-graph="${i}" class="text-[11px] text-fuchsia-400 hover:underline">🕸 ${t("net_graph")}</button>
        <details class="text-[11px]"><summary class="text-fuchsia-400 cursor-pointer hover:underline">📊 ${t("net_collective")} (${n.collective.length})</summary>
          <div class="mt-1 space-y-0.5">${n.collective.slice(0, 15).map((c) => `<div class="flex justify-between gap-2"><span ${c.contract ? `data-sm-asset="${escAttr(c.contract)}" class="cursor-pointer hover:text-accent"` : ""}>${c.symbol}${c.marketCap ? ` <span class="text-slate-500">${fmtUsd(c.marketCap)}</span>` : ""}</span><span class="text-slate-400">${c.combinedShare}% · ${c.holders} 🐋</span></div>`).join("")}</div>
        </details></div>
      </div>`).join("");
  } catch { el.innerHTML = ""; }
}

// Граф координованої мережі: диригент + кити + спільні активи в одну структуру.
export function openNetworkGraph(net: CoordNet): void {
  const modal = $("#graph-modal");
  modal.classList.remove("hidden"); modal.classList.add("flex");
  gNodes = []; gEdges = [];
  gPlayersMode = true; gRealMode = false; gFocus = null; gSelected = -1; gHover = -1; gDrag = null; gPan = null;
  gView = { scale: 0.7, ox: 0, oy: 0 };
  $("#graph-detail").classList.add("translate-x-full");
  $("#graph-empty").classList.add("hidden");
  const idx = new Map<string, number>();
  const rnd = (): number => (Math.random() - 0.5) * 280; // випадкові позиції → фізика складе «павутину»
  let condIdx = -1;
  if (net.conductor) { condIdx = gNodes.length; idx.set(net.conductor, condIdx); gNodes.push({ id: `p:${net.conductor}`, label: "🎼 " + shortAddr(net.conductor), kind: "wallet", etype: null, x: rnd(), y: rnd(), vx: 0, vy: 0, r: 16, color: "#f59e0b", deg: 1 }); }
  const ms = net.members.slice(0, 30);
  ms.forEach((m) => { idx.set(m.addr, gNodes.length); gNodes.push({ id: `p:${m.addr}`, label: shortAddr(m.addr), kind: "wallet", etype: null, x: rnd(), y: rnd(), vx: 0, vy: 0, r: 7 + Math.min(9, m.assetCount), color: WALLET_HEX, deg: m.assetCount }); });
  const sa = net.sharedAssets.slice(0, 14);
  sa.forEach((as) => { idx.set("a:" + as.symbol, gNodes.length); gNodes.push({ id: `a:${as.symbol}`, label: as.marketCap ? `${as.symbol} ${fmtUsd(as.marketCap)}` : as.symbol, kind: "entity", etype: "issuer", x: rnd(), y: rnd(), vx: 0, vy: 0, r: 9 + Math.min(26, Math.sqrt((as.marketCap || 1e6) / 1e6)), color: ENTITY_HEX.issuer, deg: 1 }); });
  if (condIdx >= 0) for (const m of ms) { const b = idx.get(m.addr); if (b != null) gEdges.push({ a: condIdx, b, w: 2 }); }
  for (const m of ms) { const a = idx.get(m.addr); for (const as of sa) { const b = idx.get("a:" + as.symbol); if (a != null && b != null) gEdges.push({ a, b, w: 1 }); } }
  modal.dataset.nodes = String(gNodes.length); modal.dataset.edges = String(gEdges.length);
  renderGraphChrome();
  graphHidden = false; cancelAnimationFrame(gRAF); gRAF = requestAnimationFrame(graphLoop);
  toast(`🕸 ${t("net_graph")}`, `${net.size} ${t("cg_members")} · ${sa.length} ${t("wc_assets")}`, "border-fuchsia-400");
}

// Граф ПОТОКУ ГРУПИ: усі члени → біржі (депозити ↗ / виведення ↘) в один об'єднаний рух коштів.
export async function openGroupFlowGraph(membersCsv: string): Promise<void> {
  const modal = $("#graph-modal");
  modal.classList.remove("hidden"); modal.classList.add("flex");
  gNodes = []; gEdges = [];
  gPlayersMode = true; gRealMode = false; gFocus = null; gSelected = -1; gHover = -1; gDrag = null; gPan = null;
  gView = { scale: 0.7, ox: 0, oy: 0 };
  $("#graph-detail").classList.add("translate-x-full");
  const empty = $("#graph-empty"); empty.classList.remove("hidden"); empty.textContent = t("mt_loading");
  try {
    const d = await getJSON<{ members: Array<{ addr: string; usd: number }>; exchanges: Array<{ name: string; depUsd: number; wdUsd: number }>; edges: Array<{ member: string; exchange: string; dir: string; usd: number }>; totalDepUsd: number; totalWdUsd: number }>("/api/group-flow", { members: membersCsv });
    gNodes = []; gEdges = [];
    const idx = new Map<string, number>();
    const rnd = (): number => (Math.random() - 0.5) * 300;
    d.members.slice(0, 20).forEach((m) => { idx.set("m:" + m.addr, gNodes.length); gNodes.push({ id: `p:${m.addr}`, label: shortAddr(m.addr), kind: "wallet", etype: null, x: rnd(), y: rnd(), vx: 0, vy: 0, r: 7 + Math.min(12, Math.sqrt((m.usd || 1) / 1e5)), color: WALLET_HEX, deg: 1 }); });
    d.exchanges.slice(0, 10).forEach((e) => { idx.set("e:" + e.name, gNodes.length); gNodes.push({ id: `cex:${e.name}`, label: `${e.name} 🏦 ${fmtUsd(e.depUsd + e.wdUsd)}`, kind: "entity", etype: "cex", x: rnd(), y: rnd(), vx: 0, vy: 0, r: 13 + Math.min(16, Math.sqrt((e.depUsd + e.wdUsd) / 1e6)), color: ENTITY_HEX.cex, deg: 1 }); });
    for (const e of d.edges) { const a = idx.get("m:" + e.member); const b = idx.get("e:" + e.exchange); if (a == null || b == null) continue; const w = Math.min(6, 1 + Math.log10(Math.max(10, e.usd)) - 1.5); if (e.dir === "dep") gEdges.push({ a, b, w }); else gEdges.push({ a: b, b: a, w }); }
    empty.classList.toggle("hidden", gNodes.length > 1);
    modal.dataset.nodes = String(gNodes.length); modal.dataset.edges = String(gEdges.length);
    renderGraphChrome();
    graphHidden = false; cancelAnimationFrame(gRAF); gRAF = requestAnimationFrame(graphLoop);
    toast(`💸 ${t("gf_btn")}`, `↗ ${t("mv_dist")} ${fmtUsd(d.totalDepUsd)} · ↘ ${t("mv_accum")} ${fmtUsd(d.totalWdUsd)}`, "border-orange-400");
  } catch { empty.textContent = t("conc_err"); }
}

// «Хто де сидить»: гаманець → усі активи, де він член координованої групи.
async function loadWhalePositions(): Promise<void> {
  const el = document.getElementById("sm-positions");
  if (!el) return;
  try {
    const d = await getJSON<{ count: number; positions: Array<{ addr: string; assetCount: number; totalShare: number; assets: Array<{ symbol: string; share: number }> }> }>("/api/whale-positions");
    const p = d.positions || [];
    if (!p.length) { el.innerHTML = ""; return; }
    el.innerHTML = `<div class="text-xs text-slate-400 mb-1 mt-3">🧭 ${t("pos_h")} <span class="text-slate-500">· ${p.length}</span></div>` +
      p.slice(0, 25).map((w) => `<div class="rounded-lg border border-edge bg-panel2 p-2 mb-1.5">
        <div class="flex justify-between items-center mb-0.5"><span data-wallet-addr="${escAttr(w.addr)}" class="text-xs font-mono cursor-pointer hover:text-accent">${shortAddr(w.addr)}</span><span class="text-[11px] text-accent">${w.assetCount} ${t("wc_assets")}</span></div>
        <div class="flex flex-wrap gap-1">${w.assets.slice(0, 12).map((a) => `<span class="text-[10px] px-1.5 py-0.5 rounded ${a.share >= 3 ? "bg-bad/15 text-bad" : a.share >= 1 ? "bg-amber-500/15 text-amber-400" : "bg-panel border border-edge text-slate-400"}">${a.symbol} ${a.share}%</span>`).join("")}</div>
      </div>`).join("");
  } catch { el.innerHTML = ""; }
}

async function loadBacktest(): Promise<void> {
  const el = document.getElementById("sm-backtest");
  if (!el) return;
  try {
    const d = await getJSON<Backtest>("/api/backtest");
    const s = d.signal;
    const running = d.status?.running ? ` <span class="text-slate-500">⟳ ${d.status.processed}/${d.status.total}</span>` : "";
    if (!s) { el.innerHTML = `<div class="text-[11px] text-slate-500 mb-3">🔬 ${t("bt_collecting")}${running}</div>`; return; }
    // Головна метрика — НАДЛИШОК над ETH. Сира дохідність ловить рух усього ринку й тому не є
    // доказом якості сигналу; вона лишається поруч дрібним шрифтом як довідка.
    const ex = s.meanExcess ?? 0;
    const exCol = ex >= 0 ? "text-good" : "text-bad";
    // Hit-rate завжди з довірчим інтервалом: якщо CI накриває 50%, сигнал статистично не відрізняється
    // від монетки — і UI має це показувати, а не малювати впевнений відсоток.
    const ci = s.hitRateCi95;
    const inconclusive = !ci || (ci[0] <= 50 && ci[1] >= 50);
    const hitCol = inconclusive ? "text-slate-300" : s.hitRate > 50 ? "text-good" : "text-bad";
    el.innerHTML = `<div class="mb-3">
      <div class="text-xs text-slate-400 mb-1.5">🔬 ${t("bt_title")}${running}</div>
      <div class="flex gap-2">
        <div class="flex-1 bg-panel2 border border-edge rounded-lg p-2">
          <div class="text-[10px] text-slate-400 mb-0.5">${t("bt_excess")}</div>
          <div class="text-sm font-semibold ${exCol}">${ex >= 0 ? "+" : ""}${ex}%</div>
          <div class="text-[11px] text-slate-500">${t("bt_median")}: ${s.medianExcess ?? "—"}% · ${t("bt_raw")}: ${s.meanRaw ?? "—"}%</div>
        </div>
        <div class="flex-1 bg-panel2 border border-edge rounded-lg p-2">
          <div class="text-[10px] text-slate-400 mb-0.5">${t("bt_hitrate")}</div>
          <div class="text-sm font-semibold ${hitCol}">${s.hitRate}% <span class="text-[11px] font-normal text-slate-500">${ci ? `(95% CI ${ci[0]}–${ci[1]})` : ""}</span></div>
          <div class="text-[11px] text-slate-500">${s.samples} ${t("bt_events")} · ${s.wallets} ${t("bt_wallets")}</div>
        </div>
      </div>
      ${inconclusive ? `<div class="text-[10px] text-amber-400/90 mt-1">⚠️ ${t("bt_inconclusive")}</div>` : ""}
      <div class="text-[10px] text-slate-500 mt-1">${t("bt_note", { h: String(d.method?.horizonDays ?? 30), b: d.method?.benchmark ?? "ETH" })}${s.pendingEvents ? ` · ${s.pendingEvents} ${t("bt_pending")}` : ""}</div>
    </div>`;
  } catch { el.innerHTML = ""; }
}

export async function openSmartMoney(): Promise<void> {
  let modal = document.getElementById("sm-modal");
  if (!modal) {
    modal = document.createElement("div");
    modal.id = "sm-modal";
    modal.className = "fixed inset-0 z-50 flex items-center justify-center p-4";
    modal.innerHTML = `<div id="sm-backdrop" class="absolute inset-0 bg-black/70"></div>
      <div class="relative bg-panel border border-edge rounded-2xl w-full max-w-2xl max-h-[88vh] overflow-y-auto p-5 shadow-2xl">
        <div class="flex items-center justify-between mb-3"><h2 class="text-base font-semibold">🐋 <span id="sm-title"></span></h2>
        <button id="sm-close" class="text-slate-400 hover:text-white text-xl leading-none">✕</button></div>
        <div id="sm-body"></div>
      </div>`;
    document.body.appendChild(modal);
    modal.querySelector("#sm-backdrop")!.addEventListener("click", closeSmartMoney);
    modal.querySelector("#sm-close")!.addEventListener("click", closeSmartMoney);
    modal.querySelector("#sm-body")!.addEventListener("click", (e) => {
      const el = e.target as HTMLElement;
      if ((el as HTMLElement).closest("[data-open-families]")) { void openFamilies(); return; }
      const ns = el.closest<HTMLElement>("[data-narr-sector]");
      if (ns?.dataset.narrSector) { smSecFilter.clear(); smSecFilter.add(ns.dataset.narrSector); rerenderAssets(); document.getElementById("sm-assets")?.scrollIntoView({ behavior: "smooth", block: "nearest" }); return; }
      const gb = el.closest<HTMLElement>("[data-sm-group]");
      if (gb?.dataset.smGroup) { smGroupBy = gb.dataset.smGroup as "sector" | "conc"; rerenderAssets(); return; }
      const sf = el.closest<HTMLElement>("[data-sm-secfilter]");
      if (sf?.dataset.smSecfilter) { const v = sf.dataset.smSecfilter; if (v === "__all") smSecFilter.clear(); else if (smSecFilter.has(v)) smSecFilter.delete(v); else smSecFilter.add(v); rerenderAssets(); return; }
      const cf = el.closest<HTMLElement>("[data-sm-concfilter]");
      if (cf?.dataset.smConcfilter) { const v = cf.dataset.smConcfilter; if (smConcFilter.has(v)) smConcFilter.delete(v); else smConcFilter.add(v); rerenderAssets(); return; }
      if (el.closest("[data-sm-graph]")) { void openPlayersGraph(); return; }
      const ng = el.closest<HTMLElement>("[data-net-graph]");
      if (ng?.dataset.netGraph) { const n = smNetworks[Number(ng.dataset.netGraph)]; if (n) { closeSmartMoney(); openNetworkGraph(n); } return; }
      const a = el.closest<HTMLElement>("[data-sm-asset]");
      if (a?.dataset.smAsset) { closeSmartMoney(); openConcentration(); $<HTMLInputElement>("#conc-input").value = a.dataset.smAsset; void runConcentration(); return; }
    });
  }
  modal.classList.remove("hidden");
  (modal.querySelector("#sm-title") as HTMLElement).textContent = t("sm_title");
  const body = modal.querySelector("#sm-body") as HTMLElement;
  body.innerHTML = `<div class="text-slate-400 text-sm py-8 text-center">${t("sm_loading")}</div>`;
  try {
    const [wa, ca, af, sf, rot, sg] = await Promise.all([
      getJSON<{ assets: Array<{ rank: number; symbol: string; contract: string; top10pct: number; band: string; sector: string }> }>("/api/whale-assets"),
      getJSON<{ multiAsset?: Array<{ address: string; count: number; totalSharePct: number; label?: string; assets: Array<{ symbol: string; share: number }> }> }>("/api/cross-asset"),
      getJSON<{ assets?: Array<{ symbol: string; rank: number; groups: Array<{ combinedShare: number; members: Array<{ addr: string; share: number }>; commonFunders: Array<{ funder: string; avgUsd: number }> }> }> }>("/api/asset-forensics").catch(() => null),
      getJSON<SectorFlow>("/api/sector-flow").catch(() => null),
      getJSON<Rotation>("/api/sector-rotation").catch(() => null),
      getJSON<SignalsData>("/api/signals").catch(() => null),
    ]);
    body.innerHTML = renderSignals(sg) + renderNarratives(sf) + renderRotation(rot) + renderSmartMoney(wa, ca, af ?? undefined);
    void loadLiquidations();
    void loadBacktest();
    void loadCoordinatedNetworks();
    void loadCoordinatedMoves();
    void loadWhalePositions();
  } catch { body.innerHTML = `<div class="text-bad text-sm py-8 text-center">${t("conc_err")}</div>`; }
}
export function closeSmartMoney(): void { document.getElementById("sm-modal")?.classList.add("hidden"); }

// Лінива статистика групи: реальний обсяг усіх гаманців + сукупні холдинги + список (клікабельний).
export async function loadGroupStats(membersCsv: string, container: HTMLElement): Promise<void> {
  container.innerHTML = `<div class="text-[11px] text-slate-500">${t("cg_loading")}</div>`;
  try {
    const d = await getJSON<{ count: number; totalUsd: number; members: Array<{ addr: string; totalUsd: number; top: string[] }>; holdings: Array<{ symbol: string; usd: number }> }>("/api/group-stats", { members: membersCsv });
    const f = (n: number) => n >= 1e9 ? "$" + (n / 1e9).toFixed(2) + "B" : n >= 1e6 ? "$" + (n / 1e6).toFixed(1) + "M" : n >= 1e3 ? "$" + (n / 1e3).toFixed(0) + "k" : "$" + Math.round(n);
    container.innerHTML = `<div class="border-t border-fuchsia-500/20 pt-1.5">
      <div class="text-xs font-semibold text-good mb-1">💰 ${t("cg_group_vol")}: ${f(d.totalUsd)} <span class="text-slate-500 font-normal">· ${d.count} ${t("cg_members")}</span></div>
      ${d.holdings.length ? `<div class="text-[10px] text-slate-400 mb-1">📦 ${t("net_collective")}: ${d.holdings.slice(0, 8).map((h) => `<b class="text-slate-300">${h.symbol}</b> ${f(h.usd)}`).join(" · ")}</div>` : ""}
      <div class="space-y-0.5">${d.members.map((m) => `<div class="flex justify-between gap-2 text-[11px]"><span data-wallet-addr="${escAttr(m.addr)}" class="font-mono cursor-pointer hover:text-accent">${shortAddr(m.addr)}</span><span class="text-slate-400">${f(m.totalUsd)} <span class="text-slate-600">${m.top.slice(0, 3).join(",")}</span></span></div>`).join("")}</div></div>`;
  } catch { container.innerHTML = `<div class="text-[11px] text-bad">${t("conc_err")}</div>`; }
}

export async function loadCapitalGroups(): Promise<void> {
  const el = document.getElementById("capital-groups");
  if (!el) return;
  el.innerHTML = `<div class="text-[11px] text-slate-500">${t("cg_loading")}</div>`;
  try {
    const [d, af, wc, hist, gdb, dg, feat, wf, cond, cnets, hlth] = await Promise.all([
      getJSON<{ groups: Array<{ size: number; members: Array<{ address: string; label: string | null; count: number }>; commonAssets: string[] }> }>("/api/capital-groups"),
      getJSON<{ status?: { cycle: number; processed: number; total: number; running: boolean }; assets: Array<{ symbol: string; rank: number; groups: Array<{ combinedShare: number; members: Array<{ addr: string; share: number }>; commonFunders: Array<{ funder: string; avgUsd: number }> }> }> }>("/api/asset-forensics").catch((): { status?: { cycle: number; processed: number; total: number; running: boolean }; assets: never[] } => ({ assets: [] })),
      getJSON<{ clusters: Array<{ id: string; label: string; combinedUsd: number; assets: string[]; assetCount: number; members: Array<{ addr: string; totalUsd: number; top: string[] }>; evidence: { mirrorPairs: number; sharedFunders: Array<{ funder: string }> } }> }>("/api/whale-clusters").catch(() => ({ clusters: [] as never[] })),
      getJSON<{ series: Array<{ t: number; count: number }> }>("/api/history", { key: "clusters" }).catch(() => ({ series: [] as never[] })),
      getJSON<{ nodes: number; edges: number; expanded: number; groups: number; volumeUsd: number } | null>("/api/graph-db-stats").catch(() => null),
      getJSON<{ groups: Array<{ size: number; volumeUsd: number; members: string[] }> }>("/api/discovered-groups").catch(() => ({ groups: [] as never[] })),
      getJSON<{ groups: Array<{ id: string; label: string; head: string; headUsd: number; note: string; assets: string[]; cashout: string; cashoutUsd: number }> }>("/api/featured-groups").catch(() => ({ groups: [] as never[] })),
      getJSON<{ groups?: Array<{ combinedUsd: number; ethAccumulator: boolean; members: Array<{ addr: string; totalUsd: number; ethShare: number }> }> }>("/api/whale-finder").catch(() => ({ groups: [] as never[] })),
      getJSON<{ conductors: Array<{ funder: string; assetCount: number; assets: string[]; holderCount: number; maxBackerUsd: number }> }>("/api/conductors").catch(() => ({ conductors: [] as never[] })),
      getJSON<{ networks: CoordNet[] }>("/api/coordinated-networks").catch(() => ({ networks: [] as never[] })),
      getJSON<{ workers: Array<{ label: string; processed: number | null; total: number | null; ageMin: number | null; ok: boolean }>; healthy: number; total: number } | null>("/api/workers-health").catch(() => null),
    ]);
    const fmtUsd = (n: number) => n >= 1e9 ? "$" + (n / 1e9).toFixed(2) + "B" : n >= 1e6 ? "$" + (n / 1e6).toFixed(1) + "M" : n >= 1e3 ? "$" + (n / 1e3).toFixed(0) + "k" : "$" + n;
    const featHtml = (feat?.groups || []).map((g) => { const lbl = /Дириген|Дериген/.test(g.label) ? `🎼 ${t("feat_conductor")}` : g.label; const note = t("feat_note").replace("%1", fmtUsd(g.headUsd)).replace("%2", String((g.assets || []).length)).replace("%3", g.cashout).replace("%4", fmtUsd(g.cashoutUsd)); return `<div class="rounded-lg border border-amber-500/40 bg-amber-500/10 p-2 mb-1.5">
        <div class="flex justify-between items-center mb-0.5"><span class="text-xs font-semibold text-amber-300">${lbl}</span><span class="text-[11px] text-amber-200">${fmtUsd(g.headUsd)}</span></div>
        <div class="text-[10px] text-slate-400 mb-1">${note}</div>
        <div class="text-[11px] text-slate-500 mb-1 truncate">${(g.assets || []).join(", ")} → <span class="text-bad">${g.cashout} ${fmtUsd(g.cashoutUsd)}</span></div>
        <div class="flex gap-1"><span data-wallet-addr="${escAttr(g.head)}" class="text-[11px] font-mono text-amber-300 cursor-pointer hover:underline">${shortAddr(g.head)}</span>
        <button data-money-trace="${escAttr(g.head)}" class="text-[11px] text-amber-400 hover:underline ml-auto">💸 ${t("mt_btn")}</button></div>
      </div>`; }).join("");
    const conds = (cond?.conductors || []).filter((c) => c.assetCount >= 2);
    const condHtml = conds.length ? `<div class="text-[11px] text-amber-300 mt-2 mb-1 font-semibold">🎼 ${t("cond_h")} <span class="text-slate-500 font-normal">${conds.length}</span></div>` + conds.slice(0, 12).map((c) => `<div class="rounded-lg border border-amber-500/30 bg-amber-500/5 p-2 mb-1.5">
        <div class="flex justify-between items-center mb-0.5"><span data-wallet-addr="${escAttr(c.funder)}" class="text-xs font-mono text-amber-300 cursor-pointer hover:underline">${shortAddr(c.funder)}</span><span class="text-[11px] text-amber-200">${c.assetCount} ${t("wc_assets")}${c.maxBackerUsd ? ` · ${fmtUsd(c.maxBackerUsd)}` : ""}</span></div>
        <div class="text-[11px] text-slate-500 mb-1 truncate">${c.assets.slice(0, 8).join(", ")} · ${c.holderCount} ${t("cg_members")}</div>
        <button data-money-trace="${escAttr(c.funder)}" class="text-[11px] text-amber-400 hover:underline">💸 ${t("mt_btn")}</button>
      </div>`).join("") : "";
    const wfGroups = (wf?.groups || []).filter((g) => g.combinedUsd >= 5e8);
    const wfHtml = wfGroups.length ? `<div class="text-[11px] text-cyan-300 mt-2 mb-1 font-semibold">🐋 ${t("wf_groups_h")}</div>` + wfGroups.slice(0, 8).map((g) => `<div class="rounded-lg border border-cyan-500/30 bg-cyan-500/5 p-2 mb-1.5">
        <div class="flex justify-between items-center"><span class="text-xs font-semibold text-cyan-300">${g.members.length} ${t("cg_members")}${g.ethAccumulator ? " · 🟦 ETH" : ""}</span><span class="text-[11px] text-cyan-200">${fmtUsd(g.combinedUsd)}</span></div>
        <div class="truncate mt-0.5">${g.members.slice(0, 6).map((m) => `<span data-wallet-addr="${escAttr(m.addr)}" class="text-[11px] font-mono text-slate-500 cursor-pointer hover:text-accent mr-1.5">${shortAddr(m.addr)}</span>`).join("")}</div>
      </div>`).join("") : "";
    const clHist = (hist?.series || []).filter((p) => typeof p.count === "number");
    const clTrend = clHist.length >= 2 ? ` ${sparkline(clHist.map((p) => p.count), "#22d3ee", 56, 16)}` : "";
    const dbLine = gdb && gdb.nodes > 0 ? `<div class="text-[10px] text-slate-500 mb-2 flex flex-wrap items-center gap-x-1.5 border-b border-edge/60 pb-1.5">🗄 ${t("gdb_base")}: <b class="text-slate-300">${gdb.nodes.toLocaleString()}</b> ${t("gdb_wallets")} · <b class="text-slate-300">${gdb.edges.toLocaleString()}</b> ${t("gdb_links")}${gdb.volumeUsd ? ` · <b class="text-good">${fmtUsd(gdb.volumeUsd)}</b> ${t("gdb_volume")}` : ""}${gdb.groups ? ` · <b class="text-cyan-300">${gdb.groups}</b> ${t("gdb_groups")}` : ""}</div>` : "";
    // КООРДИНОВАНІ МЕРЕЖІ як клікабельні групи з обсягом усієї групи (лінива статистика по кліку)
    cgNetworks = cnets?.networks || [];
    const netGroupHtml = cgNetworks.length ? `<div class="text-[11px] text-fuchsia-300 mt-1 mb-1 font-semibold">🕸 ${t("net_h")} <span class="text-slate-500 font-normal">${cgNetworks.length}</span></div>` +
      cgNetworks.slice(0, 12).map((n, i) => { const members = n.members.map((m) => m.addr).join(","); return `<div class="rounded-lg border border-fuchsia-500/30 bg-fuchsia-500/5 p-2 mb-1.5">
        <div class="flex justify-between items-center mb-0.5"><span class="text-xs font-semibold text-fuchsia-300">${t("net_one")} ${i + 1} · ${n.size} ${t("cg_members")}</span>${n.conductor ? `<span data-wallet-addr="${escAttr(n.conductor)}" class="text-[10px] text-amber-400 cursor-pointer hover:underline">🎼 ${shortAddr(n.conductor)}</span>` : ""}</div>
        ${n.sharedAssets.length ? `<div class="text-[11px] text-slate-500 mb-1 truncate">${n.sharedAssets.slice(0, 8).map((a) => a.symbol).join(", ")}</div>` : ""}
        <div class="flex gap-3 flex-wrap"><button data-group-expand="${escAttr(members)}" class="text-[11px] text-fuchsia-400 hover:underline font-medium">📂 ${t("cg_open_group")}</button>
        <button data-net-graph-cg="${i}" class="text-[11px] text-fuchsia-400 hover:underline">🕸 ${t("net_graph")}</button>
        <button data-group-flow="${escAttr(members)}" class="text-[11px] text-orange-400 hover:underline">💸 ${t("gf_btn")}</button></div>
        <div data-group-stats class="mt-1.5"></div>
      </div>`; }).join("") : "";
    const healthLine = hlth ? `<div class="text-[10px] mb-2 flex items-center gap-1.5 ${hlth.healthy === hlth.total ? "text-good" : "text-yellow-400"}" title="${hlth.workers.map((w) => `${w.label}: ${w.processed ?? "?"}/${w.total ?? "?"} (${w.ageMin ?? "?"}хв)`).join("\n")}">${hlth.healthy === hlth.total ? "🟢" : "🟡"} ${t("hp_workers")}: <b>${hlth.healthy}/${hlth.total}</b> ${t("hp_ok")}${hlth.workers.filter((w) => !w.ok).length ? ` · <span class="text-yellow-400">${hlth.workers.filter((w) => !w.ok).map((w) => w.label).join(", ")} ${t("hp_stale")}</span>` : ""}</div>` : "";
    const wcl = wc?.clusters || [];
    let html = healthLine + dbLine; /* capital-groups only: старі блоки сховано */
    if (false && wcl.length) {
      html += `<div class="text-[11px] text-cyan-300 mb-1 font-semibold flex items-center gap-1">${t("wc_h")} <span class="text-slate-500 font-normal">${wcl.length}</span>${clTrend}</div>` +
        wcl.slice(0, 12).map((c) => `<div class="rounded-lg border border-cyan-500/30 bg-cyan-500/5 p-2 mb-1.5">
          <div class="flex justify-between items-center mb-0.5"><span class="text-xs font-semibold text-cyan-300">${c.label}</span><span class="text-[11px] text-cyan-200">${fmtUsd(c.combinedUsd)}</span></div>
          <div class="text-[11px] text-slate-400 mb-1">${c.members.length} ${t("cg_members")} · ${c.assetCount} ${t("wc_assets")}${c.evidence.mirrorPairs ? ` · 🪞 ${c.evidence.mirrorPairs} ${t("wc_mirror")}` : ""}${c.evidence.sharedFunders.length ? ` · 🔗 ${c.evidence.sharedFunders.length} ${t("wc_sf")}` : ""}</div>
          ${c.assets.length ? `<div class="text-[11px] text-slate-500 mb-1 truncate">${c.assets.slice(0, 8).join(", ")}</div>` : ""}
          ${c.members.slice(0, 6).map((m) => `<div class="text-[11px] text-slate-500 truncate flex justify-between gap-2"><span data-wallet-addr="${escAttr(m.addr)}" class="font-mono cursor-pointer hover:text-accent">${shortAddr(m.addr)}</span><span class="text-slate-600">${fmtUsd(m.totalUsd)}</span></div>`).join("")}
          ${c.members.length >= 2 ? `<button data-cg-coord="${escAttr(c.members.map((m) => m.addr).join(","))}" class="w-full text-[11px] px-1.5 py-1 mt-1.5 rounded bg-fuchsia-500/15 text-fuchsia-400 border border-fuchsia-500/30 hover:bg-fuchsia-500/25 transition">${t("cg_coord")}</button><div data-cg-res class="text-[11px] mt-1 space-y-0.5"></div>` : ""}
        </div>`).join("");
    }
    const dgroups = (dg?.groups || []).filter((g) => g.members && g.members.length >= 3);
    if (false && dgroups.length) {
      html += `<div class="text-[11px] text-amber-300 mt-3 mb-1 font-semibold">🔎 ${t("dg_h")} <span class="text-slate-500 font-normal">${dgroups.length}</span></div>` +
        dgroups.slice(0, 15).map((g, i) => `<div class="rounded-lg border border-amber-500/25 bg-amber-500/5 p-2 mb-1.5">
          <div class="flex justify-between items-center mb-0.5"><span class="text-xs font-medium text-amber-300">🔎 ${t("dg_group")} ${i + 1}</span><span class="text-[11px]">${g.size} ${t("cg_members")}${g.volumeUsd ? ` · <span class="text-good">${fmtUsd(g.volumeUsd)}</span>` : ""}</span></div>
          <div class="truncate">${g.members.slice(0, 6).map((m) => `<span data-wallet-addr="${escAttr(m)}" class="text-[11px] font-mono text-slate-500 cursor-pointer hover:text-accent mr-1.5">${shortAddr(m)}</span>`).join("")}</div>
        </div>`).join("");
    }
    html += (d.groups || []).map((g, i) => `<div class="rounded-lg border border-edge bg-panel2 p-2">
        <div class="flex items-center justify-between mb-1"><span class="text-xs font-medium text-accent">🏛 ${t("cg_title")} ${i + 1}</span><span class="text-[11px] text-slate-500">${g.size} ${t("cg_members")}</span></div>
        ${g.commonAssets.length ? `<div class="text-[11px] text-slate-400 mb-1">${t("cg_assets")}: <span class="text-slate-300">${g.commonAssets.join(", ")}</span></div>` : ""}
        ${g.members.slice(0, 6).map((m) => `<div class="text-[11px] text-slate-500 truncate"><span data-wallet-addr="${escAttr(m.address)}" class="font-mono cursor-pointer hover:text-accent">${m.label || shortAddr(m.address)}</span> · ${m.count}</div>`).join("")}
        ${g.members.length >= 2 ? `<div class="flex gap-1 mt-1.5">
          <button data-cg-link="${escAttr(g.members[0].address)}|${escAttr(g.members[1].address)}" class="flex-1 text-[11px] px-1.5 py-1 rounded bg-accent/15 text-accent border border-accent/30 hover:bg-accent/25 transition">${t("cg_check")}</button>
          <button data-cg-coord="${escAttr(g.members.map((m) => m.address).join(","))}" class="flex-1 text-[11px] px-1.5 py-1 rounded bg-fuchsia-500/15 text-fuchsia-400 border border-fuchsia-500/30 hover:bg-fuchsia-500/25 transition">${t("cg_coord")}</button>
        </div><div data-cg-res class="text-[11px] mt-1 space-y-0.5"></div>` : ""}
      </div>`).join("");
    const fa = af?.assets || [];
    if (false && fa.length) {
      html += `<div class="text-[11px] text-slate-400 mt-3 mb-1 font-medium">${t("cg_forensics_h")}${af.status ? ` <span class="text-slate-500 font-normal">${af.status.running ? "⟳ " : ""}${af.status.processed}/${af.status.total}</span>` : ""}</div>` +
        fa.slice(0, 20).map((a) => `<div class="rounded-lg border border-bad/30 bg-bad/5 p-2 mb-1.5">
          <div class="flex justify-between text-xs mb-0.5"><span class="font-medium">${a.symbol} <span class="text-slate-500">#${a.rank}</span></span><span class="text-bad">${a.groups.length} ${t("sm_coord_groups")}</span></div>
          ${a.groups.slice(0, 1).map((g) => `<div class="text-[11px] text-slate-400">🏛 <b>${g.combinedShare}%</b> ${t("ag_of_supply")} · ${g.members.length} ${t("cg_members")}${g.commonFunders[0] ? ` · ${t("cg_backer")} $${g.commonFunders[0].avgUsd >= 1000 ? (g.commonFunders[0].avgUsd / 1000).toFixed(0) + "k" : g.commonFunders[0].avgUsd}` : ""}</div>
            <div class="truncate">${g.members.slice(0, 4).map((m) => `<span data-wallet-addr="${escAttr(m.addr)}" class="text-[11px] font-mono text-slate-500 cursor-pointer hover:text-accent mr-1.5">${shortAddr(m.addr)}</span>`).join("")}</div>`).join("")}
        </div>`).join("");
    }
    el.innerHTML = html || `<div class="text-[11px] text-slate-500">${t("cg_empty")}</div>`;
    /* void loadOrchestrators(); прибрано */
  } catch { el.innerHTML = `<div class="text-[11px] text-bad">${t("conc_err")}</div>`; }
}

// КРОС-АКТИВ ОРКЕСТРАТОРИ: гаманці, що тримають кілька координованих активів (не біржі) — рідкісна альфа.
async function loadOrchestrators(): Promise<void> {
  if (!BACKEND) return;
  const el = document.getElementById("capital-groups"); if (!el) return;
  try {
    const d = await getJSON<{ multiAsset: Array<{ address: string; label: string | null; count: number; assets: Array<{ symbol: string }> }> }>("/api/cross-asset");
    const list = (d.multiAsset || []).slice(0, 10);
    if (!list.length) return;
    const rows = list.map((m) => `<div class="rounded-lg border border-purple-500/30 bg-purple-500/5 p-2 mb-1.5">
      <div class="flex justify-between items-center mb-0.5"><span data-wallet-addr="${escAttr(m.address)}" class="text-xs font-mono text-purple-300 cursor-pointer hover:underline">${m.label || shortAddr(m.address)}</span><span class="text-[11px] text-purple-200">${m.count} ${t("orc_assets")}</span></div>
      <div class="text-[11px] text-slate-500 truncate">${(m.assets || []).slice(0, 8).map((a) => a.symbol).join(", ")}</div></div>`).join("");
    el.insertAdjacentHTML("afterbegin", `<div class="mb-2"><div class="text-[11px] text-purple-300 mb-1 font-semibold">🎯 ${t("orc_title")} <span class="text-slate-500 font-normal">${list.length}</span></div>${rows}</div>`);
  } catch { /* ignore */ }
}

function openConnectionGraph(path: string[]): void {
  gNodes = []; gEdges = [];
  const n = path.length;
  path.forEach((addr, i) => {
    const endpoint = i === 0 || i === n - 1;
    gNodes.push({ id: `p:${addr}`, label: shortAddr(addr), kind: endpoint ? "wallet" : "entity", etype: endpoint ? null : "unknown", x: (i - (n - 1) / 2) * 80, y: (i % 2 ? 18 : -18), vx: 0, vy: 0, r: endpoint ? 13 : 7, color: endpoint ? WALLET_HEX : ENTITY_HEX.unknown, deg: 1 });
    if (i > 0) gEdges.push({ a: i - 1, b: i, w: 2 });
  });
  gPlayersMode = true; gRealMode = false; gFocus = null; gSelected = -1; gHover = -1; gDrag = null; gPan = null;
  gView = { scale: 1, ox: 0, oy: 0 };
  const modal = $("#graph-modal");
  modal.classList.remove("hidden"); modal.classList.add("flex");
  modal.dataset.nodes = String(gNodes.length); modal.dataset.edges = String(gEdges.length);
  $("#graph-detail").classList.add("translate-x-full");
  $("#graph-empty").classList.add("hidden");
  renderGraphChrome();
  graphHidden = false;
  cancelAnimationFrame(gRAF);
  gRAF = requestAnimationFrame(graphLoop);
}

interface MoneyTrace { start: string; depthReached: number; totalNodes: number; edges: number; edgesList: Array<{ from: string; to: string; usd: number; depth: number }>; cexExits: Array<{ from: string; to: string; exchange: string; asset: string; amount: number; usd: number; depth: number }>; cexExitTotalUsd: number; cexWithdrawals: Array<{ from: string; to: string; exchange: string; asset: string; amount: number; usd: number; depth: number }>; cexWithdrawalTotalUsd: number; }
function depthColor(dp: number, max: number): string {
  const t2 = dp / (max || 1);
  return `rgb(${Math.round(59 + t2 * 180)},${Math.round(130 - t2 * 62)},${Math.round(246 - t2 * 178)})`; // синій → червоний за глибиною
}
// Візуалізація потоку грошей: диригент + усі коліна + точка кешауту (Binance) в один граф.
export async function openMoneyTraceGraph(address: string, label?: string): Promise<void> {
  closePortfolio();
  const addr = address.toLowerCase();
  const modal = $("#graph-modal");
  modal.classList.remove("hidden"); modal.classList.add("flex");
  $("#graph-empty").classList.remove("hidden"); $("#graph-empty").textContent = t("mt_loading");
  gNodes = []; gEdges = [];
  gPlayersMode = true; gRealMode = false; gFocus = null; gSelected = -1; gHover = -1; gDrag = null; gPan = null;
  gView = { scale: 0.55, ox: 0, oy: 0 };
  $("#graph-detail").classList.add("translate-x-full");
  graphHidden = false; cancelAnimationFrame(gRAF); gRAF = requestAnimationFrame(graphLoop);
  try {
    const d = await getJSON<MoneyTrace>("/api/money-trace", { address: addr, depth: 15 });
    const edges = (d.edgesList || []).slice().sort((a, b) => b.usd - a.usd).slice(0, 120);
    const depthOf = new Map<string, number>([[addr, 0]]);
    for (const e of edges) { if (!depthOf.has(e.from)) depthOf.set(e.from, e.depth - 1); if (!depthOf.has(e.to)) depthOf.set(e.to, e.depth); }
    const maxDepth = Math.max(1, ...[...depthOf.values()]);
    const byDepth = new Map<number, string[]>();
    for (const [a, dp] of depthOf) { const arr = byDepth.get(dp) || []; arr.push(a); byDepth.set(dp, arr); }
    const idx = new Map<string, number>();
    for (const [dp, addrs] of byDepth) addrs.forEach((a, i) => {
      idx.set(a, gNodes.length);
      const isStart = a === addr;
      gNodes.push({ id: `p:${a}`, label: isStart ? (label || shortAddr(a)) : shortAddr(a), kind: "wallet", etype: null, x: (dp - maxDepth / 2) * 78, y: (i - (addrs.length - 1) / 2) * 32, vx: 0, vy: 0, r: isStart ? 15 : 7, color: isStart ? "#f59e0b" : depthColor(dp, maxDepth), deg: 1 });
    });
    for (const e of edges) { const a = idx.get(e.from), b = idx.get(e.to); if (a != null && b != null) gEdges.push({ a, b, w: Math.min(4, 1 + Math.log10(Math.max(10, e.usd)) - 2) }); }
    // ОКРЕМИЙ вузол на КОЖНУ біржу (за назвою), розмір за сумою припливу
    const exTotal = new Map<string, number>();
    for (const c of (d.cexExits || [])) exTotal.set(c.exchange, (exTotal.get(c.exchange) || 0) + c.usd);
    for (const c of (d.cexWithdrawals || [])) exTotal.set(c.exchange, (exTotal.get(c.exchange) || 0) + c.usd);
    const exNodes = new Map<string, number>();
    let ei = 0;
    for (const [name, total] of [...exTotal.entries()].sort((a, b) => b[1] - a[1])) {
      exNodes.set(name, gNodes.length);
      gNodes.push({ id: `cex:${name}`, label: `${name} 🏦 ${fmtUsd(total)}`, kind: "entity", etype: "cex", x: (maxDepth / 2 + 1.9) * 78, y: (ei - (exTotal.size - 1) / 2) * 80, vx: 0, vy: 0, r: 14 + Math.min(12, Math.sqrt(total / 1e6)), color: ENTITY_HEX.cex, deg: 1 });
      ei++;
    }
    for (const c of (d.cexExits || [])) { const a = idx.get(c.from); const b = exNodes.get(c.exchange); if (a != null && b != null) gEdges.push({ a, b, w: Math.min(6, 1 + Math.log10(Math.max(10, c.usd)) - 1.5) }); }
    for (const c of (d.cexWithdrawals || [])) { const a = exNodes.get(c.exchange); const b = idx.get(c.to); if (a != null && b != null) gEdges.push({ a, b, w: Math.min(6, 1 + Math.log10(Math.max(10, c.usd)) - 1.5) }); }
    $("#graph-empty").classList.toggle("hidden", gNodes.length > 1);
    modal.dataset.nodes = String(gNodes.length); modal.dataset.edges = String(gEdges.length);
    renderGraphChrome();
    renderMoneyExits(d); // список виходів на біржу: яка біржа, який актив, скільки
    const topEx = [...exTotal.entries()].sort((a, b) => b[1] - a[1])[0];
    toast(`💸 ${t("mt_btn")}`, `${d.depthReached} ${t("mt_hops")} · ${d.totalNodes} ${t("gdb_wallets")} · ${fmtUsd(d.cexExitTotalUsd)} → ${topEx ? topEx[0] : t("mt_exchange")}`, "border-yellow-400");
  } catch { $("#graph-empty").textContent = t("conc_err"); }
}

// Потоки біржі: депозити (вихід на біржу) + виведення (накопичення) — біржа → актив → к-сть → $.
function renderMoneyExits(d: MoneyTrace): void {
  const panel = $("#graph-detail");
  const deposits = d.cexExits || [], withdrawals = d.cexWithdrawals || [];
  if (!deposits.length && !withdrawals.length) { panel.classList.add("translate-x-full"); return; }
  const section = (title: string, color: string, items: MoneyTrace["cexExits"], total: number, dir: string): string => {
    if (!items.length) return "";
    const byEx = new Map<string, MoneyTrace["cexExits"]>();
    for (const c of items) { const arr = byEx.get(c.exchange) || []; arr.push(c); byEx.set(c.exchange, arr); }
    const sorted = [...byEx.entries()].map(([name, list]) => ({ name, list, total: list.reduce((s, c) => s + c.usd, 0) })).sort((a, b) => b.total - a.total);
    return `<div class="text-sm font-semibold ${color} mb-1 mt-3">${title} <span class="text-slate-400 font-normal">${fmtUsd(total)}</span></div>` +
      sorted.map((ex) => `<div class="mb-2">
        <div class="text-xs font-semibold text-slate-200 mb-1">🏦 ${ex.name} <span class="${color}">${fmtUsd(ex.total)}</span></div>
        ${ex.list.slice().sort((a, b) => b.usd - a.usd).slice(0, 10).map((c) => `<div class="flex justify-between gap-2 text-[11px] py-0.5 border-b border-edge/40">
          <span>${dir} <span class="text-accent font-medium">${c.asset}</span> <span class="text-slate-400">${c.amount.toLocaleString(undefined, { maximumFractionDigits: 2 })}</span></span>
          <span class="text-slate-300">${c.usd ? fmtUsd(c.usd) : "—"}</span></div>`).join("")}
      </div>`).join("");
  };
  // ЧИСТИЙ ПОТІК per-актив: виведення − депозити. + = накопичує, − = скидає.
  const netSection = (): string => {
    const net = new Map<string, { dep: number; wd: number }>();
    for (const c of deposits) { const e = net.get(c.asset) || { dep: 0, wd: 0 }; e.dep += c.usd; net.set(c.asset, e); }
    for (const c of withdrawals) { const e = net.get(c.asset) || { dep: 0, wd: 0 }; e.wd += c.usd; net.set(c.asset, e); }
    const arr = [...net.entries()].map(([asset, e]) => ({ asset, net: e.wd - e.dep })).filter((x) => Math.abs(x.net) >= 1).sort((a, b) => Math.abs(b.net) - Math.abs(a.net));
    if (!arr.length) return "";
    return `<div class="text-sm font-semibold text-slate-200 mb-1 mt-3">📊 ${t("mt_net")}</div>` +
      arr.slice(0, 14).map((x) => { const pos = x.net >= 0; return `<div class="flex justify-between gap-2 text-[11px] py-0.5 border-b border-edge/40">
        <span><span class="text-accent font-medium">${x.asset}</span> <span class="${pos ? "text-good" : "text-bad"}">${pos ? "↘ " + t("mt_accum") : "↗ " + t("mt_dump")}</span></span>
        <span class="${pos ? "text-good" : "text-bad"} font-semibold">${pos ? "+" : "−"}${fmtUsd(Math.abs(x.net))}</span></div>`; }).join("");
  };
  panel.innerHTML = `<div class="flex items-center justify-between mb-1"><div class="text-sm font-semibold text-slate-200">🏦 ${t("mt_flows")}</div><button id="gd-close" class="text-slate-400 hover:text-white text-lg leading-none">✕</button></div>`
    + netSection()
    + section(`💸 ${t("mt_exits")}`, "text-yellow-400", deposits, d.cexExitTotalUsd, "↗")
    + section(`💰 ${t("mt_withdrawals")}`, "text-good", withdrawals, d.cexWithdrawalTotalUsd, "↘");
  panel.classList.remove("translate-x-full");
  panel.querySelector("#gd-close")?.addEventListener("click", () => panel.classList.add("translate-x-full"));
}

export async function checkGroupLink(from: string, to: string, resEl: HTMLElement): Promise<void> {
  resEl.textContent = t("cg_checking");
  try {
    const d = await getJSON<{ found: boolean; path?: string[]; hops?: number; pattern?: { type: string; note: string } }>("/api/connection-path", { from, to, maxHops: 15 });
    if (d.found && d.path) {
      const pat = d.pattern && (d.pattern.type === "peel_chain" || d.pattern.type === "gas_funding") ? ` · <span class="${d.pattern.type === "peel_chain" ? "text-bad font-semibold" : "text-yellow-400"}">${t("cg_pat_" + d.pattern.type)}</span>` : "";
      resEl.innerHTML = `<span class="text-good">🔗 ${t("cg_linked")} ${d.hops} ${t("cg_hops")}</span>${pat}`;
      openConnectionGraph(d.path);
    } else { resEl.innerHTML = `<span class="text-slate-500">${t("cg_nolink")}</span>`; }
  } catch { resEl.textContent = t("conc_err"); }
}

export async function checkGroupCoordination(addresses: string, resEl: HTMLElement): Promise<void> {
  resEl.textContent = t("cg_checking");
  try {
    const d = await getJSON<{ score: number; commonFunders: Array<{ funder: string; count: number; type: string; avgUsd: number }>; directFunding: unknown[] }>("/api/group-coordination", { addresses });
    const verdict = d.score >= 60
      ? `<span class="text-bad font-semibold">⚠ ${t("cg_likely")} · ${d.score}</span>`
      : `<span class="text-slate-400">${t("cg_weak")} · ${d.score}</span>`;
    const usdFmt = (u: number) => (u >= 1000 ? "$" + (u / 1000).toFixed(1) + "k" : "$" + u);
    resEl.innerHTML = `${verdict}<div class="text-slate-400">${d.commonFunders.length} ${t("cg_common")} · ${d.directFunding.length} ${t("cg_direct")}</div>` +
      d.commonFunders.slice(0, 4).map((c) => `<div class="text-slate-500 truncate">↳ <span data-wallet-addr="${escAttr(c.funder)}" class="font-mono cursor-pointer hover:text-accent">${shortAddr(c.funder)}</span> → ${c.count} <span class="${c.type === "value" ? "text-bad" : "text-slate-600"}">${t("cg_funder_" + c.type)}${c.avgUsd ? " " + usdFmt(c.avgUsd) : ""}</span></div>`).join("");
  } catch { resEl.textContent = t("conc_err"); }
}

export async function openPlayersGraph(): Promise<void> {
  const ca = await getJSON<{ multiAsset?: Array<{ address: string; count: number; label?: string; assets: Array<{ symbol: string; share: number }> }> }>("/api/cross-asset").catch(() => null);
  const players = (ca?.multiAsset || []).slice(0, 25);
  if (!players.length) return;
  closeSmartMoney();
  gNodes = []; gEdges = [];
  const assetIdx = new Map<string, number>();
  const ensureAsset = (sym: string): number => {
    if (assetIdx.has(sym)) return assetIdx.get(sym)!;
    const i = gNodes.length;
    const ang = Math.random() * Math.PI * 2, rad = 140 + Math.random() * 60;
    gNodes.push({ id: `a:${sym}`, label: sym, kind: "entity", etype: "issuer", x: Math.cos(ang) * rad, y: Math.sin(ang) * rad, vx: 0, vy: 0, r: 9, color: ENTITY_HEX.issuer, deg: 0 });
    assetIdx.set(sym, i); return i;
  };
  players.forEach((p, pi) => {
    const ang = (pi / players.length) * Math.PI * 2;
    const pIdx = gNodes.length;
    gNodes.push({ id: `p:${p.address}`, label: p.label || shortAddr(p.address), kind: "wallet", etype: null, x: Math.cos(ang) * 50, y: Math.sin(ang) * 50, vx: 0, vy: 0, r: 8 + Math.min(12, p.count), color: WALLET_HEX, deg: p.count });
    p.assets.forEach((a) => { const ai = ensureAsset(a.symbol); gNodes[ai].deg += 1; gEdges.push({ a: pIdx, b: ai, w: Math.max(1, Math.round(a.share)) }); });
  });
  for (const n of gNodes) if (n.kind === "entity") n.r = 7 + Math.min(14, n.deg * 2);
  gPlayersMode = true; gRealMode = false; gFocus = null; gSelected = -1; gHover = -1; gDrag = null; gPan = null;
  gView = { scale: 1, ox: 0, oy: 0 };
  const modal = $("#graph-modal");
  modal.classList.remove("hidden"); modal.classList.add("flex");
  modal.dataset.nodes = String(gNodes.length); modal.dataset.edges = String(gEdges.length);
  $("#graph-detail").classList.add("translate-x-full");
  $("#graph-empty").classList.add("hidden");
  renderGraphChrome();
  graphHidden = false;
  cancelAnimationFrame(gRAF);
  gRAF = requestAnimationFrame(graphLoop);
}

export function openGraph(): void {
  // За замовчуванням показуємо ОДИН гаманець на РЕАЛЬНИХ ретроданих блокчейну; «← Усі зв'язки» — подієвий граф.
  const first = state.wallets[0];
  gSelected = -1; gHover = -1; gDrag = null; gPan = null; gPlayersMode = false;
  gView = { scale: 1, ox: 0, oy: 0 };
  gNodes = []; gEdges = [];
  const modal = $("#graph-modal");
  modal.classList.remove("hidden"); modal.classList.add("flex");
  $("#graph-detail").classList.add("translate-x-full");
  populateGraphWalletSelect();
  graphHidden = false;
  cancelAnimationFrame(gRAF);
  gRAF = requestAnimationFrame(graphLoop);
  if (first) { void loadWalletGraph(first); }
  else { gFocus = null; gRealMode = false; buildGraphData(); updateFocusUI(); renderGraphChrome(); }
}

export function closeGraph(): void {
  cancelAnimationFrame(gRAF); gRAF = 0;
  const modal = $("#graph-modal");
  modal.classList.add("hidden"); modal.classList.remove("flex");
  graphHidden = true;
}

// Реальний граф гаманця з ретроданих блокчейну (усі мережі) — будує gNodes/gEdges.
export async function loadRealGraph(address: string, label: string): Promise<boolean> {
  const seq = ++gLoadSeq;
  try {
    const d = await getJSON<{
      nodes: Array<{ id: string; kind: "wallet" | "entity"; etype: EntityType | null; label: string; count?: number; verified?: boolean }>;
      edges: Array<{ from: string; to: string; count: number }>;
    }>("/api/wallet-graph", { address });
    if (seq !== gLoadSeq) return false; // застаріла відповідь — інший гаманець уже завантажується
    if (!d.nodes || d.nodes.length < 2) return false;
    const n = d.nodes.length;
    gNodes = d.nodes.map((nd, i): GNode => {
      const ang = (i / Math.max(1, n)) * Math.PI * 2;
      const rad = nd.kind === "wallet" ? 0 : 70 + (i % 6) * 26;
      return {
        id: nd.id, label: nd.kind === "wallet" ? label : (nd.verified ? "✓ " + nd.label : nd.label), kind: nd.kind, etype: nd.etype ?? null, verified: nd.verified,
        x: Math.cos(ang) * rad, y: Math.sin(ang) * rad, vx: 0, vy: 0,
        r: (nd.kind === "wallet" ? 13 : 6) + Math.min(13, Math.sqrt(nd.count ?? 1) * 3),
        color: nd.kind === "wallet" ? WALLET_HEX : ENTITY_HEX[nd.etype ?? "unknown"], deg: nd.count ?? 1,
      };
    });
    const im = new Map(gNodes.map((nd, i) => [nd.id, i]));
    gEdges = (d.edges || []).map((e) => ({ a: im.get(e.from) ?? -1, b: im.get(e.to) ?? -1, w: e.count })).filter((e) => e.a >= 0 && e.b >= 0);
    gRealMode = true;
    const modal = $("#graph-modal");
    modal.dataset.nodes = String(gNodes.length);
    modal.dataset.edges = String(gEdges.length);
    $("#graph-empty").classList.add("hidden");
    renderGraphChrome();
    return true;
  } catch { return false; }
}

async function loadWalletGraph(wallet: Wallet): Promise<void> {
  gFocus = { type: "wallet", key: wallet.id };
  syncGraphWalletSelect();
  updateFocusUI();
  const empty = $("#graph-empty");
  empty.textContent = ({ uk: "Завантаження графа з блокчейну…", en: "Loading on-chain graph…", de: "Lade On-Chain-Graph…", fr: "Chargement du graphe on-chain…", ru: "Загрузка графа из блокчейна…" } as Record<Lang, string>)[lang];
  empty.classList.remove("hidden");
  gNodes = []; gEdges = [];
  let ok = false;
  if (BACKEND && wallet.address && wallet.address.length > 25) ok = await loadRealGraph(wallet.address, wallet.label || shortAddr(wallet.address));
  if (!ok) { gRealMode = false; buildGraphData(); renderGraphChrome(); }
  // Ні ончейн-графа, ні локальних подій: замінити «Завантаження…» на підсумок,
  // інакше напис лишається назавжди і виглядає як завислий запит.
  if (!gNodes.length) empty.textContent = t("graph_empty");
  empty.classList.toggle("hidden", gNodes.length > 0);
  const modal = $("#graph-modal");
  modal.dataset.nodes = String(gNodes.length);
  modal.dataset.edges = String(gEdges.length);
  gSelected = gNodes.findIndex((nd) => nd.kind === "wallet");
}

// Витягти адресу гаманця з id вузла (різні режими графа)
function nodeAddress(nd: GNode): string {
  if (nd.id.startsWith("p:") || nd.id.startsWith("u:")) return nd.id.slice(2);
  if (nd.id.startsWith("a:") || nd.id.startsWith("e:")) return ""; // актив/сутність за назвою — без адреси
  if (nd.id.startsWith("w:")) { const w = state.wallets.find((x) => x.id === nd.id.slice(2)); return w?.address || ""; }
  return nd.id; // реальний граф: id вузла = адреса
}

// Панель деталей вузла у реальному/гравцевому графі: Arkham-картка + граф цього гаманця
function openRealNodeDetail(idx: number): void {
  const nd = gNodes[idx];
  if (!nd) return;
  gSelected = idx;
  const addr = nodeAddress(nd);
  const panel = $("#graph-detail");
  panel.innerHTML = `
    <div class="flex items-center justify-between mb-2"><div class="text-sm font-medium flex items-center gap-2"><span class="w-3 h-3 rounded-full inline-block" style="background:${nd.color}"></span>${nd.label}</div><button id="gd-close" class="text-slate-400 hover:text-white text-lg leading-none">✕</button></div>
    ${nd.verified ? `<div class="mb-2 inline-flex items-center gap-1 text-[11px] px-2 py-0.5 rounded-full bg-good/15 text-good border border-good/30">✓ ${t("gd_verified")}${nd.etype ? ` · ${t("entity_" + nd.etype)}` : ""}</div>` : ""}
    ${addr && addr.length >= 26 ? `<div class="font-mono text-[11px] text-slate-400 break-all mb-1">${addr}</div>
      <div class="text-xs mb-2"><a href="${arkhamLink(addr)}" target="_blank" rel="noopener" class="text-purple-400 hover:underline">🔍 Arkham →</a></div>
      <button id="gd-trace" class="w-full mb-2 bg-amber-500 hover:bg-amber-400 text-black font-semibold text-sm py-2 rounded-xl transition">💸 ${t("mt_btn")}</button>
      <div id="gd-netflow" class="mb-2"></div>
      <div class="grid grid-cols-2 gap-2 mb-2"><button id="gd-card" class="bg-accent hover:bg-blue-500 text-white text-xs py-2 rounded-xl transition">📊 ${t("gd_card")}</button>
      <button id="gd-sub" class="bg-panel2 hover:border-accent border border-edge text-xs py-2 rounded-xl transition">🕸 ${t("gd_subgraph")}</button></div>
      <div class="text-[11px] text-slate-400 mt-2 mb-1">💼 ${t("gd_holdings")}</div>
      <div id="gd-holdings" class="text-[11px] text-slate-500">${t("conc_loading")}</div>
      <div class="text-[10px] text-slate-500 mt-2">${t("gd_hint")}</div>`
    : `<div class="text-xs text-slate-500">${nd.id.startsWith("a:") ? t("gd_asset") : "—"}</div>`}`;
  panel.classList.remove("translate-x-full");
  panel.querySelector("#gd-close")?.addEventListener("click", () => panel.classList.add("translate-x-full"));
  panel.querySelector("#gd-card")?.addEventListener("click", () => openWalletAddress(addr, nd.label));
  panel.querySelector("#gd-trace")?.addEventListener("click", () => { panel.classList.add("translate-x-full"); void openMoneyTraceGraph(addr, nd.label); });
  panel.querySelector("#gd-sub")?.addEventListener("click", () => { gView = { scale: 1, ox: 0, oy: 0 }; panel.classList.add("translate-x-full"); void loadRealGraph(addr, nd.label); });
  // вміст гаманця (об'єм + к-сть активів + топ) + вердикт накопичує/скидає — прямо в панелі
  if (addr && addr.length >= 26 && /^0x[0-9a-fA-F]{40}$/.test(addr)) {
    void getJSON<{ totalUsd: number; tokens: Array<{ symbol: string; valueUsd: number }> }>("/api/wallet-assets", { address: addr }).then((d) => {
      const h = document.getElementById("gd-holdings");
      if (!h) return;
      const toks = (d.tokens || []).filter((tk) => tk.valueUsd > 0).slice(0, 12);
      h.innerHTML = toks.length ? `<div class="text-slate-200 font-medium mb-1">💰 ${fmtUsd(d.totalUsd)} · 📦 ${toks.length}+ ${t("pf_assets").toLowerCase()}</div>` + toks.map((tk) => `<div class="flex justify-between gap-2"><span>${tk.symbol}</span><span class="text-slate-400">${fmtUsd(tk.valueUsd)}</span></div>`).join("") : `<div class="text-slate-500">—</div>`;
    }).catch(() => { const h = document.getElementById("gd-holdings"); if (h) h.textContent = "—"; });
    void getJSON<{ netflows: Array<{ asset: string; net: number }> }>("/api/wallet-netflow", { address: addr }).then((nfd) => {
      const ne = document.getElementById("gd-netflow"); if (!ne) return;
      const nf = nfd.netflows || [];
      ne.innerHTML = nf.slice(0, 4).map((x) => { const pos = x.net >= 0; return `<span class="inline-flex items-center gap-1 text-[10px] px-2 py-0.5 rounded-full mr-1 mb-1 ${pos ? "bg-good/15 text-good border border-good/30" : "bg-bad/15 text-bad border border-bad/30"}">${pos ? "📈 " + t("mt_accum") : "📉 " + t("mt_dump")} <b>${x.asset}</b> ${pos ? "+" : "−"}${fmtUsd(Math.abs(x.net))}</span>`; }).join("");
    }).catch(() => { });
  }
}

export function setupGraphInteractions(): void {
  const canvas = $<HTMLCanvasElement>("#graph-canvas");
  const tip = $("#graph-tooltip");
  const pt = (e: MouseEvent): { mx: number; my: number } => {
    const r = canvas.getBoundingClientRect();
    return { mx: e.clientX - r.left, my: e.clientY - r.top };
  };
  const worldOf = (mx: number, my: number): { wx: number; wy: number } => {
    const r = canvas.getBoundingClientRect();
    return { wx: (mx - (r.width / 2 + gView.ox)) / gView.scale, wy: (my - (r.height / 2 + gView.oy)) / gView.scale };
  };

  canvas.addEventListener("mousedown", (e: MouseEvent) => {
    const { mx, my } = pt(e);
    const idx = graphPick(mx, my);
    gDownX = mx; gDownY = my; gMoved = false; gDownIdx = idx; gPointerActive = true;
    if (idx >= 0) { gDrag = { idx }; canvas.style.cursor = "grabbing"; }
    else { gPan = { x: e.clientX, y: e.clientY }; canvas.style.cursor = "move"; }
  });
  window.addEventListener("mousemove", (e: MouseEvent) => {
    if (graphHidden) return;
    const { mx, my } = pt(e);
    if ((gDrag || gPan) && Math.hypot(mx - gDownX, my - gDownY) > 4) gMoved = true;
    if (gDrag) {
      const { wx, wy } = worldOf(mx, my);
      const nd = gNodes[gDrag.idx];
      nd.x = wx; nd.y = wy; nd.vx = 0; nd.vy = 0;
      tip.classList.add("hidden");
    } else if (gPan) {
      gView.ox += e.clientX - gPan.x; gView.oy += e.clientY - gPan.y;
      gPan = { x: e.clientX, y: e.clientY };
    } else {
      gHover = graphPick(mx, my);
      if (gHover >= 0) {
        const nd = gNodes[gHover];
        tip.innerHTML = `<div class="font-medium">${nd.label}</div><div class="text-slate-400">${nd.kind === "wallet" ? t("wallets") : t(`entity_${nd.etype}`)} · ${nd.deg} ${t("graph_links")}</div>`;
        tip.style.left = `${mx + 14}px`; tip.style.top = `${my + 14}px`;
        tip.classList.remove("hidden");
        canvas.style.cursor = "grab";
      } else { tip.classList.add("hidden"); canvas.style.cursor = "default"; }
    }
  });
  window.addEventListener("mouseup", () => {
    if (gPointerActive && !graphHidden && !gMoved) {
      if (gDownIdx >= 0) {
        gSelected = gDownIdx;
        if (gRealMode || gPlayersMode) openRealNodeDetail(gDownIdx); // одинарний клік → панель деталей
        else openNodeDetail(gDownIdx);
      } else if (!gRealMode && !gPlayersMode) closeNodeDetail();
    }
    gPointerActive = false; gDrag = null; gPan = null; gDownIdx = -1; canvas.style.cursor = "default";
  });
  // ПОДВІЙНИЙ клік по вузлу → перейти в граф цього гаманця
  canvas.addEventListener("dblclick", (e: MouseEvent) => {
    if (graphHidden) return;
    const { mx, my } = pt(e);
    const idx = graphPick(mx, my);
    if (idx < 0) return;
    const nd = gNodes[idx];
    const addr = nodeAddress(nd);
    if (!addr || addr.length < 26) return;
    gView = { scale: 1, ox: 0, oy: 0 };
    $("#graph-detail").classList.add("translate-x-full");
    void loadRealGraph(addr, nd.label);
  });
  canvas.addEventListener("wheel", (e: WheelEvent) => {
    e.preventDefault();
    const { mx, my } = pt(e);
    const { wx, wy } = worldOf(mx, my);
    const r = canvas.getBoundingClientRect();
    gView.scale = Math.max(0.3, Math.min(3, gView.scale * (e.deltaY < 0 ? 1.1 : 0.9)));
    gView.ox = mx - r.width / 2 - wx * gView.scale;
    gView.oy = my - r.height / 2 - wy * gView.scale;
  }, { passive: false });
}

/* ---------- Бічна панель деталей вузла + фокус ---------- */

function walletDetailHtml(wid: string): string {
  const w = state.wallets.find((x) => x.id === wid);
  if (!w) return "";
  const net0 = networkById(w.networks[0]);
  const walletEvents = state.events.filter((e) => e.walletId === wid);
  const conns = new Map<string, { label: string; type: EntityType; count: number }>();
  walletEvents.forEach((e) => {
    const key = entityKeyOf(e);
    const c = conns.get(key) || { label: e.counterpartyEntity.type === "unknown" ? shortAddr(e.counterparty) : e.counterpartyEntity.name, type: e.counterpartyEntity.type, count: 0 };
    c.count += 1; conns.set(key, c);
  });
  const list = [...conns.values()].sort((a, b) => b.count - a.count).slice(0, 12);
  const chips = w.networks.map((id) => { const n = networkById(id); return `<span class="text-xs ${n.color}">${n.name}</span>`; }).join('<span class="text-slate-600 text-xs"> · </span>');
  const focused = gFocus?.type === "wallet" && gFocus.key === wid;
  return `
    <div class="text-base font-semibold flex items-center gap-2"><span class="w-3 h-3 rounded-full inline-block" style="background:${WALLET_HEX}"></span>${w.label || shortAddr(w.address)}</div>
    <div class="font-mono text-xs text-slate-300 break-all mt-1">${w.address}</div>
    <div class="flex flex-wrap gap-3 mt-2 text-xs">
      ${link(explorerAddr(net0, w.address), t("btn_explorer"))}
      <a href="${arkhamLink(w.address)}" target="_blank" rel="noopener" class="text-purple-400 hover:underline">🔍 Arkham Intel →</a>
    </div>
    <div class="grid grid-cols-2 gap-2 mt-3">
      <div class="bg-panel2 rounded-lg p-2"><div class="text-[11px] text-slate-400">${t("g_balance")}</div><div class="text-sm font-medium">${fmtUsd(w.valueUsd)}</div></div>
      <div class="bg-panel2 rounded-lg p-2"><div class="text-[11px] text-slate-400">${t("g_events")}</div><div class="text-sm font-medium">${walletEvents.length}</div></div>
    </div>
    <div class="mt-3"><div class="text-[11px] text-slate-400 mb-1">${t("g_networks")}</div><div class="flex flex-wrap gap-1.5">${chips}</div></div>
    <div class="mt-3">
      <div class="text-[11px] text-slate-400 mb-1">${t("g_holdings")}</div>
      <div class="space-y-1">
        ${w.holdings.map((h) => `<div class="flex items-center justify-between text-xs"><span class="text-slate-300 font-mono">${fmtNum(h.amount, 4)} ${h.asset}</span><span class="text-slate-400">${fmtUsd(h.amount * priceOf(h.asset))}</span></div>`).join("") || `<div class="text-xs text-slate-500">—</div>`}
      </div>
    </div>
    <div class="mt-3">
      <div class="text-[11px] text-slate-400 mb-1">${t("g_connected_entities")} (${conns.size})</div>
      <div class="space-y-1 max-h-52 overflow-y-auto pr-1">
        ${list.map((c) => `<div class="flex items-center justify-between gap-2 text-xs"><span class="px-2 py-0.5 rounded-full ${ENTITY_TYPE_CLS[c.type]} truncate">${c.label}</span><span class="text-slate-500">${c.count}</span></div>`).join("") || `<div class="text-xs text-slate-500">—</div>`}
      </div>
    </div>
    <button data-portfolio-wallet="${wid}" class="w-full mt-3 bg-panel2 hover:border-accent border border-edge text-sm py-2 rounded-xl transition">📊 ${t("pf_open")}</button>
    <button data-focus-wallet="${wid}" class="w-full mt-2 ${focused ? "bg-slate-600" : "bg-accent hover:bg-blue-500"} text-white text-sm font-medium py-2 rounded-xl transition">🕸 ${t("g_focus")}</button>`;
}

function entityDetailHtml(nd: GNode): string {
  const conns = new Map<string, number>();
  state.events.filter((e) => entityKeyOf(e) === nd.id).forEach((e) => conns.set(e.walletId, (conns.get(e.walletId) || 0) + 1));
  const wlist = [...conns.entries()]
    .map(([wid, count]) => ({ w: state.wallets.find((x) => x.id === wid), count }))
    .filter((x) => x.w).sort((a, b) => b.count - a.count);
  const addr = nd.etype === "unknown" ? nd.id.slice(2) : nd.label;
  return `
    <div class="text-base font-semibold flex items-center gap-2"><span class="w-3 h-3 rounded-full inline-block" style="background:${nd.color}"></span>${nd.label}</div>
    <div class="mt-1 text-xs">${t(`entity_${nd.etype}`)}</div>
    <div class="mt-2 text-xs"><a href="${arkhamLink(addr)}" target="_blank" rel="noopener" class="text-purple-400 hover:underline">🔍 Arkham Intel →</a></div>
    ${nd.etype && nd.etype !== "unknown" ? `<button data-entity-name="${escAttr(nd.label)}" data-entity-type="${nd.etype}" class="w-full mt-3 bg-panel2 hover:border-accent border border-edge text-sm py-2 rounded-xl transition">${t("ent_profile_btn")}</button>` : ""}
    <div class="mt-3">
      <div class="text-[11px] text-slate-400 mb-1">${t("g_connected_wallets")} (${wlist.length})</div>
      <div class="space-y-1 max-h-72 overflow-y-auto pr-1">
        ${wlist.map((x) => `<button data-focus-wallet="${x.w!.id}" class="w-full flex items-center justify-between gap-2 text-xs bg-panel2 hover:border-accent border border-edge rounded-lg px-2 py-1.5 transition"><span class="truncate text-left">${x.w!.label || shortAddr(x.w!.address)}</span><span class="text-slate-500">${x.count} →</span></button>`).join("") || `<div class="text-xs text-slate-500">—</div>`}
      </div>
    </div>`;
}

export function openNodeDetail(idx: number): void {
  if (idx < 0 || idx >= gNodes.length) return;
  const nd = gNodes[idx];
  gSelected = idx;
  const title = nd.kind === "wallet" ? t("g_wallet_one") : t("g_entity_one");
  const body = nd.kind === "wallet" ? walletDetailHtml(nd.id.slice(2)) : entityDetailHtml(nd);
  $("#graph-detail").innerHTML = `
    <div class="flex items-center justify-between mb-3">
      <span class="text-xs uppercase tracking-wide text-slate-400">${title}</span>
      <button id="graph-detail-close" class="text-slate-400 hover:text-slate-200 text-xl leading-none">×</button>
    </div>
    ${body}`;
  $("#graph-detail").classList.remove("translate-x-full");
}

export function closeNodeDetail(): void {
  gSelected = -1;
  $("#graph-detail").classList.add("translate-x-full");
}

export function updateFocusUI(): void {
  const btn = $("#graph-showall");
  if (gFocus) { btn.textContent = t("g_show_all"); btn.classList.remove("hidden"); }
  else btn.classList.add("hidden");
}

/** Заповнює випадайку гаманців у вікні графа. */
export function populateGraphWalletSelect(): void {
  const sel = $<HTMLSelectElement>("#graph-wallet-select");
  const ext = gGraphExtWallets.map((w) => `<option value="${w.id}">${w.label || shortAddr(w.address)} ★</option>`).join("");
  sel.innerHTML = `<option value="">${t("g_all_wallets")}</option>` +
    state.wallets.map((w) => `<option value="${w.id}">${w.label || shortAddr(w.address)}</option>`).join("") + ext;
  syncGraphWalletSelect();
}

export function syncGraphWalletSelect(): void {
  const sel = $<HTMLSelectElement>("#graph-wallet-select");
  sel.value = gFocus?.type === "wallet" ? gFocus.key : "";
  document.getElementById("graph-wallet-del")?.classList.toggle("hidden", !sel.value);
}

/** Прибрати поточний вибраний гаманець зі списку графа. */
export function removeGraphWallet(): void {
  const id = $<HTMLSelectElement>("#graph-wallet-select").value;
  if (!id) return;
  if (gGraphExtWallets.some((w) => w.id === id)) gGraphExtWallets = gGraphExtWallets.filter((w) => w.id !== id);
  else nav.removeWallet(id); // відстежуваний гаманець — прибрати й зі списку гаманців
  populateGraphWalletSelect();
  clearFocus(); // повернутись до перегляду «усі гаманці»
}

export function focusWallet(wid: string, showDetail = true): void {
  const wallet = state.wallets.find((w) => w.id === wid) || gGraphExtWallets.find((w) => w.id === wid);
  if (!wallet) return;
  gView = { scale: 1, ox: 0, oy: 0 };
  $("#graph-detail").classList.add("translate-x-full");
  void loadWalletGraph(wallet).then(() => {
    if (showDetail && !gRealMode) { const idx = gNodes.findIndex((n) => n.id === `w:${wid}`); if (idx >= 0) openNodeDetail(idx); }
  });
}

export function clearFocus(): void {
  gFocus = null;
  gRealMode = false;
  gPlayersMode = false;
  gView = { scale: 1, ox: 0, oy: 0 };
  buildGraphData();
  $("#graph-modal").dataset.nodes = String(gNodes.length);
  $("#graph-modal").dataset.edges = String(gEdges.length);
  updateFocusUI();
  syncGraphWalletSelect();
  renderGraphChrome();
  closeNodeDetail();
}


/* ---- refresh-hooks для каскадів композиційного кореня ---- */
export function refreshNodeOnUpdate(): void {
  if (!graphHidden && !gRealMode && gSelected >= 0) openNodeDetail(gSelected);
}
export function refreshGraphLang(): void {
  if (graphHidden) return;
  populateGraphWalletSelect(); renderGraphChrome(); updateFocusUI();
  if (!gRealMode && gSelected >= 0) openNodeDetail(gSelected);
}

/** Мережа з вкладки «Групи капіталу» за індексом (для делегованих обробників). */
export const cgNetworkAt = (i: number): CoordNet | undefined => cgNetworks[i];
