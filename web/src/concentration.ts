/* ---------- Концентрація власників токена (публічні дані) ----------
   Самодостатня фіча: власна модалка + під-панелі (asset-flow, стратегія,
   netflow, тренд, holder-assets, ринковий контекст, Token-AI). Приватний стан
   lastConc/concAssets/currentProfile. Жодних відкривачів інших фіч. */

import { t, clarityShortL } from "./i18n";
import { universeByContract, sectorEmoji } from "./universe";
import { $, fmtUsd, localeOf, shortAddr } from "./util";
import { escAttr, toast } from "./ui";
import { getJSON } from "./api";
import { BACKEND } from "./env";
import { arkhamLink } from "./attribution";
import { clarityBadge } from "./clarity";
import { openAssetHolders } from "./holders";


interface ConcHolder { rank: number; address: string; share: number; cum: number; cls?: string; label?: string; }
interface ConcScores { top10: number; entry: number; fearGreed: number; behaviour: number; }
interface ConcData { token: { name: string; symbol: string; holdersCount: number; contract: string }; holders: ConcHolder[]; breakdown?: Record<string, { count: number; sharePct: number }>; reach: { p40: number | null; p60: number | null; p70: number | null }; topCumNote: number; scores?: ConcScores; }

const CLASS_META: Record<string, { icon: string; key: string; cls: string }> = {
  whale: { icon: "🐋", key: "class_whale", cls: "text-blue-400 bg-blue-400/10" },
  cex: { icon: "🏦", key: "class_cex", cls: "text-yellow-400 bg-yellow-400/10" },
  mm: { icon: "🤖", key: "class_mm", cls: "text-pink-400 bg-pink-400/10" },
  fund: { icon: "🏛", key: "class_fund", cls: "text-fuchsia-400 bg-fuchsia-400/10" },
  burn: { icon: "🔥", key: "class_burn", cls: "text-red-400 bg-red-400/10" },
  contract: { icon: "📜", key: "class_contract", cls: "text-purple-400 bg-purple-400/10" },
  retail: { icon: "👤", key: "class_retail", cls: "text-slate-400 bg-slate-500/10" },
  large: { icon: "🔷", key: "class_large", cls: "text-sky-400 bg-sky-400/10" },
  holder: { icon: "👤", key: "class_holder", cls: "text-slate-400 bg-slate-500/10" },
};

/** Напівкруглий датчик 0..100 (SVG). */
function gaugeSvg(value: number, label: string): string {
  const v = Math.max(0, Math.min(100, value));
  const a = Math.PI * (v / 100);
  const x = (50 - 40 * Math.cos(a)).toFixed(1), y = (50 - 40 * Math.sin(a)).toFixed(1);
  const color = v >= 66 ? "#22c55e" : v >= 33 ? "#eab308" : "#ef4444";
  return `<div class="text-center">
    <svg viewBox="0 0 100 56" class="w-full">
      <path d="M 10 50 A 40 40 0 0 1 90 50" fill="none" stroke="#27324a" stroke-width="9" stroke-linecap="round"/>
      <path d="M 10 50 A 40 40 0 0 1 ${x} ${y}" fill="none" stroke="${color}" stroke-width="9" stroke-linecap="round"/>
    </svg>
    <div class="text-base font-semibold -mt-2" style="color:${color}">${v}</div>
    <div class="text-[10px] text-slate-400 leading-tight">${label}</div>
  </div>`;
}

let lastConc: ConcData | null = null;
const concAssets: Record<string, { symbol: string; valueUsd: number }[]> = {};

export function openConcentration(): void {
  $("#conc-body").innerHTML = `<div class="text-sm text-slate-500">${t("conc_hint")}</div>`;
  const m = $("#conc-modal"); m.classList.remove("hidden"); m.classList.add("flex");
}
export function closeConcentration(): void { const m = $("#conc-modal"); m.classList.add("hidden"); m.classList.remove("flex"); }

// Instant light card from our own universe (in-memory) — always shows the asset
// (name / sector / rank / top10-50 / price) the moment you click, without waiting
// on the slow live-Blockscout holder fetch. Holders load below.
function lightCard(contract: string): string {
  const u = universeByContract(contract);
  const name = u?.name || contract;
  const sym = u?.symbol || "";
  const sec = u && u.sector !== "other"
    ? `<span class="px-1.5 py-0.5 rounded bg-panel border border-edge text-[11px]">${sectorEmoji(u.sector)} ${t("sec_" + u.sector)}</span>` : "";
  const rank = u?.rank ? `<span class="text-slate-400">${t("um_rank")} <span class="text-slate-200 font-medium">#${u.rank}</span></span>` : "";
  const conc = u && (u.top10pct != null || u.top50pct != null)
    ? `<div class="text-xs text-slate-400 mt-0.5">${t("intel_top10")}: <span class="text-slate-200 font-medium">${u.top10pct ?? "—"}%</span> · ${t("conc_top50")}: <span class="text-slate-200">${u.top50pct ?? "—"}%</span></div>` : "";
  return `<div class="bg-panel2 border border-edge rounded-xl p-3">
      <div class="font-semibold">${name} <span class="text-slate-400">${sym}</span></div>
      <div class="flex items-center gap-2 mt-1 text-xs">${rank}${sec}<span class="font-mono text-[10px] text-slate-500 truncate">${contract}</span></div>
      ${conc}
      <div id="conc-price" class="text-xs text-slate-400 mt-0.5"></div>
    </div>
    <div id="conc-backtest"></div>
    <div id="conc-networks"></div>
    <div id="conc-holders-slot" class="text-sm text-slate-400 py-3">${t("conc_loading")}</div>`;
}

export async function runConcentration(): Promise<void> {
  const contract = $<HTMLInputElement>("#conc-input").value.trim();
  const body = $("#conc-body");
  if (!/^0x[0-9a-fA-F]{40}$/.test(contract)) { body.innerHTML = `<div class="text-sm text-bad">${t("conc_bad")}</div>`; return; }
  // Instant info first — never blocks on Blockscout.
  body.innerHTML = lightCard(contract);
  const u0 = universeByContract(contract);
  void loadAssetPrice(contract);
  void loadAssetNetworks(contract, u0?.symbol || "");
  void loadAssetBacktest(u0?.symbol || "");
  try {
    const d = await getJSON<ConcData>("/api/concentration", { contract });
    lastConc = d;
    renderConcentration(lastConc);
    void loadAssetFlow(contract); // графік ціни + середні точки входу/виходу китів
    void loadConcTrend(d.token?.symbol); // тренд концентрації (історія)
    void loadNetflow(contract); // netflow біржі (накопичення vs роздача)
    void loadStrategy(contract); // стратегія DCA-за-китом (фаза, E=48% вихід, план траншів)
    void loadAssetBacktest(d.token?.symbol || "");
    void loadAssetNetworks(contract, d.token?.symbol || "");
    void loadAssetPrice(contract);
  } catch (e) {
    const slot = document.getElementById("conc-holders-slot");
    if (slot) slot.innerHTML = `<div class="text-sm text-bad">${t("conc_err")}: ${(e as Error).message}</div>`;
  }
}

// ── Стратегія DCA-за-китом (фаза, E=48% вихід, план траншів) ──
async function loadStrategy(contract: string): Promise<void> {
  const el = document.getElementById("conc-strategy");
  if (!el) return;
  el.innerHTML = `<div class="bg-panel2 border border-edge rounded-xl p-3"><div class="text-[11px] text-slate-500">🎯 Аналіз стратегії…</div></div>`;
  try {
    const d = await getJSON<{ exitRatio: number; color: string; phase: string; action: string; currentPrice: number; avgEntry: { price: number } | null; dca: { entry: Array<{ tranche: number; pct: number }>; exit: Array<{ atE: number; sellTo: number; done: boolean; final: boolean }> } }>("/api/strategy", { contract });
    if (document.getElementById("conc-strategy")) el.innerHTML = strategyCardHtml(d);
  } catch (e) {
    el.innerHTML = `<div class="bg-panel2 border border-edge rounded-xl p-3 text-[11px] text-slate-500">🎯 Стратегія недоступна: ${(e as Error).message}</div>`;
  }
}

function strategyCardHtml(d: { exitRatio: number; color: string; phase: string; action: string; currentPrice: number; avgEntry: { price: number } | null; dca: { entry: Array<{ tranche: number; pct: number }>; exit: Array<{ atE: number; sellTo: number; done: boolean; final: boolean }> } }): string {
  const E = d.exitRatio || 0;
  const ePos = Math.min(100, (E / 48) * 100);
  const phaseLabel: Record<string, string> = { accumulation: "Накопичення", markup: "Розгін", distribution: "Роздача", exit: "Вихід", neutral: "Нейтрально" };
  const entry = (d.dca?.entry || []).map((tr) => `<span class="px-1.5 py-0.5 rounded bg-good/15 text-good text-[10px]">DCA ${tr.tranche} · ${tr.pct}%</span>`).join(" ");
  const exitLadder = (d.dca?.exit || []).map((s) => `<div class="flex items-center justify-between text-[11px] py-0.5 ${s.done ? "text-slate-200" : "text-slate-500"}"><span>${s.done ? "✓" : "•"} E ≥ ${s.atE}%</span><span class="${s.final ? "text-bad font-medium" : ""}">→ продано ${s.sellTo}%${s.final ? " · вихід" : ""}</span></div>`).join("");
  const ref = d.avgEntry?.price ? `Сер. вхід китів: <span class="text-slate-300">${fmtUsd(d.avgEntry.price)}</span>${d.currentPrice ? ` · зараз ${fmtUsd(d.currentPrice)}` : ""}` : "";
  return `<div class="bg-panel2 border rounded-xl p-3" style="border-color:${d.color}55">
    <div class="flex items-center justify-between mb-1.5">
      <div class="font-semibold text-sm">🎯 Стратегія DCA-за-китом</div>
      <span class="text-[10px] px-2 py-0.5 rounded-full" style="background:${d.color}22;color:${d.color}">${phaseLabel[d.phase] || d.phase}</span>
    </div>
    <div class="text-[12px] mb-2" style="color:${d.color}">${d.action}</div>
    <div class="text-[10px] text-slate-400 mb-0.5">Вихід емісії (E): <span class="font-medium" style="color:${d.color}">${E}%</span> / поріг виходу 48%</div>
    <div class="relative h-2 rounded bg-panel mb-2">
      <div class="absolute inset-y-0 left-0 rounded" style="width:${ePos}%;background:${d.color}"></div>
      <div class="absolute inset-y-0" style="right:0;width:2px;background:#f87171" title="48% — остаточний вихід"></div>
    </div>
    <div class="text-[10px] text-slate-500 mb-1">DCA-IN (накопичення слідом за китом):</div>
    <div class="flex flex-wrap gap-1 mb-2">${entry}</div>
    <div class="text-[10px] text-slate-500 mb-0.5">DCA-OUT (роздача → остаточний вихід при 48%):</div>
    ${exitLadder}
    ${ref ? `<div class="text-[10px] text-slate-500 mt-1.5 pt-1.5 border-t border-edge/50">${ref}</div>` : ""}
  </div>`;
}

// ── Інституційна стратегія (Core-Satellite) ──
interface PHolding { sym: string; weight: number; clarity: string; sector: string; status: string; phase?: string }
interface PSleeve { target: number; role: string; holdings: PHolding[] }
/** Ешелон каскаду: фундамент → середні → малі → хайризик. */
interface PTier { tier: string; title: string; targetPct: number; actualPct: number; names: number; holdings: PHolding[] }
/** Смартмані — вітрина спостереження, а не портфель: суми в ній не мають сенсу. */
const isShowcase = () => currentProfile === "smartmoney";
interface PProfile { id: string; name: string; emoji: string }
interface PData { cascade?: PTier[]; profile: string; profileEmoji: string; profileDesc: string; profiles: PProfile[]; exitE: number; caps: { maxCore: number; maxSat: number }; allowContested: boolean; inspiredBy: string; sleeves: { core: PSleeve; satellite: PSleeve; stable: PSleeve }; risk: { names: number; maxWeightPct: number } }
let currentProfile = "moderate";
/** Капітал для розрахунку сум. $100k — типовий приклад, змінюється в полі. */
let stratCapital = 100000;
/** Останні дані стратегії — щоб перерахунок сум не смикав сервер. */
let lastPortfolio: PData | null = null;

function money(v: number): string {
  if (v >= 1000) return "$" + Math.round(v).toLocaleString("uk-UA").replace(/\u00a0/g, " ");
  return "$" + v.toFixed(v >= 100 ? 0 : 1);
}

function phaseTag(phase?: string): string {
  const m: Record<string, [string, string, string]> = { accumulation: ["ph_accum", "#34d399", "🟢"], markup: ["ph_markup", "#5b9dff", "🔵"], distribution: ["ph_distr", "#fb923c", "🟠"], exit: ["ph_exit", "#f87171", "🔴"] };
  const v = phase && m[phase]; if (!v) return "";
  return `<span class="text-[10px] ml-1" style="color:${v[1]}" title="DCA">${v[2]} ${t(v[0])}</span>`;
}

// ── Ринковий контекст (макро-шар: F&G, домінація, TVL → режим) ──
interface MRotation { ethBtcRatio: number | null; btcDomChange: number | null; ethDomChange: number | null; phase: "btc_to_eth" | "eth_to_btc" | "neutral"; onchain: { accumulate: number; distribute: number; bias: string } | null; haveHistory: boolean }
interface MCtx { fearValue: number; fearClass: string; mcap: number; btcDom: number; ethDom?: number; change24: number; tvl: number; regime: string; rotation?: MRotation; reasons: string[] }
function rotationRowHtml(r: MRotation): string {
  const map: Record<string, [string, string]> = { btc_to_eth: ["🔄", "#34d399"], eth_to_btc: ["🔁", "#fb923c"], neutral: ["➖", "#94a3b8"] };
  const [ic, col] = map[r.phase] || map.neutral;
  const label = r.haveHistory ? t("mc_rot_" + r.phase) : t("mc_rot_collecting");
  const dch = (r.haveHistory && r.btcDomChange != null && r.ethDomChange != null)
    ? ` <span class="text-slate-500">(BTC.d ${r.btcDomChange > 0 ? "+" : ""}${r.btcDomChange}пп · ETH.d ${r.ethDomChange > 0 ? "+" : ""}${r.ethDomChange}пп/3д)</span>` : "";
  const oc = r.onchain?.bias === "accumulate" ? ` · <span style="color:#34d399">${t("mc_rot_onchain_acc")} ${r.onchain.accumulate}/${r.onchain.distribute}</span>`
    : r.onchain?.bias === "distribute" ? ` · <span style="color:#fb923c">${t("mc_rot_onchain_dist")} ${r.onchain.distribute}/${r.onchain.accumulate}</span>` : "";
  return `<div class="text-[11px] mt-1"><span style="color:${col}">${ic} ${t("mc_rot")}: <b>${label}</b></span>${dch}${oc}</div>`;
}
function marketCtxHtml(d: MCtx): string {
  const reg: Record<string, [string, string]> = { accumulate: ["🟢", "#34d399"], neutral: ["⚪", "#94a3b8"], derisk: ["🔴", "#f87171"] };
  const [icon, col] = reg[d.regime] || reg.neutral;
  const fearCol = d.fearValue <= 25 ? "#f87171" : d.fearValue >= 75 ? "#fb923c" : "#5b9dff";
  const chCol = (d.change24 ?? 0) >= 0 ? "#34d399" : "#f87171";
  return `<div class="bg-panel2 border rounded-xl p-3" style="border-color:${col}55">
    <div class="flex items-center justify-between mb-1.5">
      <span class="text-sm font-semibold">🌐 ${t("mc_title")}</span>
      <span class="text-xs px-2 py-0.5 rounded-full" style="background:${col}22;color:${col}">${icon} ${t("mc_" + d.regime)}</span>
    </div>
    <div class="flex flex-wrap gap-x-4 gap-y-1 text-[11px] text-slate-300 mb-1.5">
      <span>${t("mc_fear")}: <b style="color:${fearCol}">${d.fearValue} · ${d.fearClass}</b></span>
      <span>BTC.d: <b>${d.btcDom != null ? d.btcDom.toFixed(1) : "—"}%</b></span>
      ${d.ethDom != null ? `<span>ETH.d: <b>${d.ethDom.toFixed(1)}%</b></span>` : ""}
      <span>${t("mc_mcap")}: <b>$${d.mcap ? (d.mcap / 1e12).toFixed(2) : "—"}T</b> <span style="color:${chCol}">${(d.change24 ?? 0) >= 0 ? "+" : ""}${d.change24 != null ? d.change24.toFixed(1) : "—"}%</span></span>
      <span>TVL: <b>$${d.tvl ? (d.tvl / 1e9).toFixed(0) : "—"}B</b></span>
    </div>
    <div class="text-[11px]" style="color:${col}">${t("mc_tilt_" + d.regime)}</div>
    ${d.rotation ? rotationRowHtml(d.rotation) : ""}
  </div>`;
}
async function loadMarketContext(): Promise<void> {
  const el = document.getElementById("market-ctx"); if (!el || !BACKEND) return;
  try { const d = await getJSON<MCtx & { error?: unknown }>("/api/market-context"); if (d && !d.error && document.getElementById("market-ctx")) el.innerHTML = marketCtxHtml(d); } catch { /* ignore */ }
}

export async function openPortfolioStrategy(profile = currentProfile): Promise<void> {
  currentProfile = profile;
  const m = document.getElementById("strat-modal"); if (!m) return;
  m.classList.remove("hidden"); m.classList.add("flex");
  const body = document.getElementById("strat-body"); if (!body) return;
  body.innerHTML = `<div class="text-sm text-slate-500">${t("strat_building")}</div>`;
  try {
    const d = await getJSON<PData>("/api/portfolio-strategy", { profile });
    lastPortfolio = d;
    body.innerHTML = portfolioHtml(d);
    wirePortfolio(body);
    void loadMarketContext();
  } catch (e) { body.innerHTML = `<div class="text-sm text-bad">${(e as Error).message}</div>`; }
}
export function closePortfolioStrategy(): void {
  const m = document.getElementById("strat-modal"); if (m) { m.classList.add("hidden"); m.classList.remove("flex"); }
}

/** Обробники картки стратегії. Винесено, бо зміна капіталу перемальовує розмітку
 *  локально — без повторного запиту на сервер. */
function wirePortfolio(body: HTMLElement): void {
  body.querySelectorAll<HTMLElement>("[data-prof]").forEach((b) =>
    b.addEventListener("click", () => void openPortfolioStrategy(b.dataset.prof || "moderate")));
  // клік на актив веде до холдерів — далі адреса відкриває гаманець
  body.querySelectorAll<HTMLElement>("[data-strat-asset]").forEach((b) =>
    b.addEventListener("click", () => void openAssetHolders(b.dataset.stratAsset || "")));
  document.getElementById("pf-backtest-btn")?.addEventListener("click", () => void loadPortfolioBacktest());

  // зміна капіталу перераховує суми локально: дані вже завантажені
  const capInput = body.querySelector<HTMLInputElement>("#strat-capital");
  capInput?.addEventListener("input", () => {
    const v = Number(capInput.value);
    if (!Number.isFinite(v) || v < 1) return;
    stratCapital = v;
    if (!lastPortfolio) return;
    const pos = capInput.selectionStart;
    body.innerHTML = portfolioHtml(lastPortfolio);
    wirePortfolio(body);
    const again = body.querySelector<HTMLInputElement>("#strat-capital");
    if (again) {
      again.focus();
      again.setSelectionRange(pos ?? again.value.length, pos ?? again.value.length);
    }
  });
}

function sleeveHtml(name: string, role: string, color: string, sl: PSleeve): string {
  const rows = sl.holdings.map((h) => `<button type="button" data-strat-asset="${h.sym}"
      class="w-full text-left flex items-center gap-2 py-1 text-xs hover:bg-white/5 rounded px-1 -mx-1 group">
      <span class="w-24 font-medium shrink-0">${h.sym}${phaseTag(h.phase)}</span>
      <div class="flex-1 h-2 rounded bg-panel"><div class="h-2 rounded" style="width:${Math.min(100, (h.weight / (sl.target || 1)) * 100)}%;background:${color}"></div></div>
      <span class="w-12 text-right shrink-0">${h.weight}%</span>
      ${`<span class="w-20 text-right shrink-0 font-medium text-emerald-300">${money((stratCapital * h.weight) / 100)}</span>`}
      ${(h as PHolding & { emissionShare?: number; liquidityTrap?: boolean }).emissionShare
        ? `<span class="w-16 text-right shrink-0 text-[10px] ${(h as PHolding & { liquidityTrap?: boolean }).liquidityTrap ? "text-rose-400" : "text-amber-300"}"
                 title="частка емісії, яку тримає група капіталу${(h as PHolding & { liquidityTrap?: boolean }).liquidityTrap ? " — понад 70%: позицію не продати без обвалу" : ""}">
             ${(h as PHolding & { emissionShare?: number }).emissionShare}% емісії${(h as PHolding & { liquidityTrap?: boolean }).liquidityTrap ? " ⚠" : ""}</span>`
        : ""}
      <span class="text-[10px] text-slate-400 w-24 truncate shrink-0">${clarityShortL(h.status)} · ${h.sector}</span>
      <span class="text-[10px] text-slate-600 opacity-0 group-hover:opacity-100 shrink-0">холдери →</span></button>`).join("");
  return `<div class="bg-panel2 border border-edge rounded-xl p-3 mb-2" style="border-left:3px solid ${color}">
      <div class="flex items-center justify-between mb-1"><span class="font-semibold text-sm" style="color:${color}">${name}</span><span class="text-xs text-slate-400">${sl.holdings.length} поз. · ${sl.target}%${isShowcase() ? "" : ` · <b class="text-emerald-300">${money((stratCapital * sl.target) / 100)}</b>`}</span></div>
      <div class="text-[11px] text-slate-400 mb-2">${role}</div>${rows}</div>`;
}

/** Кольори ешелонів: від спокійного зеленого фундаменту до червоного хайризику. */
const TIER_COLOR: Record<string, string> = {
  large: "#34d399",
  mid: "#5b9dff",
  small: "#fbbf24",
  risk: "#f87171",
};

/** Усі позиції портфеля — з каскаду, а якщо його немає, зі старих рукавів. */
function allHoldings(d: PData): PHolding[] {
  if (d.cascade && d.cascade.length) return d.cascade.flatMap((x) => x.holdings);
  const sl = d.sleeves as unknown as { core?: PSleeve; satellite?: PSleeve };
  return [...(sl.core?.holdings ?? []), ...(sl.satellite?.holdings ?? [])];
}

/**
 * Каскад: чотири ешелони за ринковою капіталізацією.
 *
 * Кожен ешелон показує СВОЮ суму в грошах, а не лише відсоток. Портфель у відсотках
 * читається як абстракція; той самий портфель, підписаний у доларах, одразу показує,
 * де позиція завелика для заявленого капіталу, а де дрібна настільки, що комісії
 * зʼїдять результат.
 */
function cascadeHtml(d: PData): string {
  if (!d.cascade || !d.cascade.length) {
    const sl = d.sleeves as unknown as { core?: PSleeve; satellite?: PSleeve };
    return sl.core ? sleeveHtml(t("strat_core"), t("strat_core_role"), "#34d399", sl.core) : "";
  }
  return d.cascade
    .filter((x) => x.holdings.length)
    .map((x) => {
      const color = TIER_COLOR[x.tier] ?? "#94a3b8";
      const cash = (stratCapital * x.actualPct) / 100;
      const head = `${x.title} · <span style="color:${color}">${x.actualPct}%</span>` +
        ` · <b class="text-emerald-300">${money(cash)}</b>` +
        ` · ${x.names} поз.`;
      const sl: PSleeve = { target: x.actualPct || x.targetPct, role: "", holdings: x.holdings };
      return sleeveHtml(head, tierRole(x.tier), color, sl);
    })
    .join("");
}

function tierRole(tier: string): string {
  switch (tier) {
    case "large": return "Фундамент. Велика капіталізація, активи під наглядом кількох груп капіталу — звідси починається каскад.";
    case "mid": return "Середня капіталізація. Куди групи йдуть за дохідністю, коли фундамент уже набрано.";
    case "small": return "Мала капіталізація. Вища віддача, але вихід вужчий — розмір позиції обмежено навмисно.";
    case "risk": return "Хайризик. Мікрокапіталізація, переважно AI. Коротко- й середньостроково, малими частками.";
    default: return "";
  }
}

function portfolioHtml(d: PData): string {
  const tabs = d.profiles.map((p) => `<button data-prof="${p.id}" class="flex-1 text-xs py-1.5 rounded-lg border transition ${p.id === d.profile ? "bg-accent/20 border-accent/50 text-accent font-medium" : "bg-panel2 border-edge text-slate-400 hover:border-accent"}">${p.emoji} ${t("prof_" + p.id)}</button>`).join("");
  const rules = [
    t("strat_r_cap").replace("{a}", String(d.caps.maxCore)).replace("{b}", String(d.caps.maxSat)),
    d.allowContested ? t("strat_r_excl_aggr") : t("strat_r_excl"),
    t("strat_r_conc"), t("strat_r_riskoff"),
    t("strat_r_exit").replace("{a}", String(d.exitE)),
  ];
  const factors = [t("strat_f_flow"), t("strat_f_quality"), t("strat_f_reg"), t("strat_f_liq")];
  return `<div id="market-ctx" class="mb-3"></div>
    <div class="flex gap-1.5 mb-3">${tabs}</div>
    <div class="text-[12px] text-slate-300 bg-panel2 border border-edge rounded-xl p-3 mb-3">${d.profileEmoji} <b>${t("prof_" + d.profile)}:</b> ${t("prof_" + d.profile + "_desc")}</div>
    <div class="text-[11px] text-slate-500 mb-3">💡 ${t("strat_inspired")}</div>
    ${(() => {
      const hs = allHoldings(d);
      const sum = hs.reduce((n, h) => n + h.weight, 0);
      const minPos = hs.length ? Math.min(...hs.map((h) => h.weight)) : 0;
      if (isShowcase()) {
        return `<div class="bg-panel2 border border-edge rounded-xl p-3 mb-3">
          <div class="flex items-center gap-2 flex-wrap mb-2">
            <label class="text-xs text-slate-400">Капітал:</label>
            <div class="flex items-center gap-1">
              <span class="text-xs text-slate-500">$</span>
              <input id="strat-capital" type="number" min="100" step="1000" value="${stratCapital}"
                     class="bg-panel border border-edge rounded-lg px-2 py-1 text-xs w-28" />
            </div>
            <span class="ml-auto text-[11px] text-slate-500">${hs.length} позицій · розподілено <b class="text-slate-300">${sum.toFixed(1)}%</b></span>
          </div>
          <div class="text-xs text-slate-300 mb-1">Вітрина спостереження — не інструкція до покупки</div>
          <div class="text-[11px] text-slate-500">
            ${hs.length} активів, за якими система стежить. Позначені <b class="text-slate-300">★</b> увійшли
            в портфельні стратегії. Головна колонка тут — <b class="text-amber-300">частка емісії</b>:
            скільки активу тримає одна група капіталу.
          </div>
        </div>`;
      }
      return `<div class="bg-panel2 border border-edge rounded-xl p-3 mb-3">
        <div class="flex items-center gap-2 flex-wrap">
          <label class="text-xs text-slate-400">Капітал:</label>
          <div class="flex items-center gap-1">
            <span class="text-xs text-slate-500">$</span>
            <input id="strat-capital" type="number" min="100" step="1000" value="${stratCapital}"
                   class="bg-panel border border-edge rounded-lg px-2 py-1 text-xs w-28" />
          </div>
          <span class="ml-auto text-[11px] text-slate-500">
            ${hs.length} позицій · розподілено <b class="text-slate-300">${sum.toFixed(1)}%</b>
          </span>
        </div>
        <div class="text-[11px] text-slate-500 mt-2">
          Найменша позиція — ${minPos.toFixed(1)}% (${money((stratCapital * minPos) / 100)}).
          Стейблкоїни у стратегію не входять: кеш тримається поза нею.
        </div>
      </div>`;
    })()}
    ${cascadeHtml(d)}
    <div class="bg-panel2 border border-edge rounded-xl p-3 mb-2">
      <div class="font-semibold text-sm mb-1">${t("strat_factors")}</div>
      ${factors.map((f) => `<div class="text-xs text-slate-300 mt-0.5">▸ ${f}</div>`).join("")}
    </div>
    <div class="bg-panel2 border border-edge rounded-xl p-3 mb-2">
      <div class="font-semibold text-sm mb-1">${t("strat_risk")} · ${d.risk.names} ${t("strat_assets")} · ${t("strat_maxw")} ${d.risk.maxWeightPct}%</div>
      ${rules.map((r) => `<div class="text-xs text-slate-300 mt-0.5">• ${r}</div>`).join("")}
      <div class="text-[11px] text-slate-500 mt-2">${t("strat_rebalance")}</div>
    </div>
    <button id="pf-backtest-btn" class="w-full bg-accent/15 hover:bg-accent/25 text-accent border border-accent/30 text-sm py-2 rounded-lg transition">${t("strat_bt_btn")}</button>
    <div id="pf-backtest-result" class="mt-2"></div>`;
}

function equityChart(eq: { static: number[]; dynamic: number[]; btc: number[] | null }): string {
  const series: Array<[number[], string]> = [];
  if (eq.btc?.length) series.push([eq.btc, "#fbbf24"]);
  if (eq.static?.length) series.push([eq.static, "#64748b"]);
  if (eq.dynamic?.length) series.push([eq.dynamic, "#5b9dff"]);
  if (!series.length) return "";
  const all = series.flatMap(([a]) => a); const mx = Math.max(...all), mn = Math.min(...all);
  const W = 320, H = 70, pad = 3;
  const path = (arr: number[]) => arr.map((v, i) => `${((i / Math.max(1, arr.length - 1)) * W).toFixed(1)},${(H - pad - ((v - mn) / ((mx - mn) || 1)) * (H - 2 * pad)).toFixed(1)}`).join(" ");
  const lines = series.map(([a, c]) => `<polyline points="${path(a)}" fill="none" stroke="${c}" stroke-width="1.5"/>`).join("");
  const dot = (c: string, lbl: string) => `<span style="display:inline-flex;align-items:center;gap:3px"><span style="width:7px;height:2px;background:${c};display:inline-block"></span>${lbl}</span>`;
  return `<svg viewBox="0 0 ${W} ${H}" preserveAspectRatio="none" class="w-full" style="height:64px">${lines}</svg>
    <div class="flex gap-3 text-[9px] text-slate-400 mt-0.5 justify-center">${eq.btc ? dot("#fbbf24", "BTC") : ""}${dot("#64748b", t("strat_bt_static"))}${dot("#5b9dff", t("strat_bt_dynamic"))}</div>`;
}

async function loadPortfolioBacktest(): Promise<void> {
  const el = document.getElementById("pf-backtest-result"); if (!el) return;
  el.innerHTML = `<div class="text-xs text-slate-500 py-2">${t("strat_bt_calc")}</div>`;
  try {
    type BtMetrics = { totalRet: number; cagr: number; maxDD: number; volAnn: number; sharpe: number | null };
    const d = await getJSON<{ error?: string; benchmarks: Record<string, BtMetrics>; period: { years: number; months: number }; assets: unknown[]; equity?: { static: number[]; dynamic: number[]; btc: number[] | null }; portfolio: BtMetrics; portfolioDynamic?: BtMetrics }>("/api/portfolio-backtest", { profile: currentProfile });
    if (d.error) { el.innerHTML = `<div class="text-xs text-bad">${d.error}</div>`; return; }
    const row = (name: string, m: { totalRet: number; cagr: number; maxDD: number; volAnn: number; sharpe: number | null }, hi = false) => `<tr class="${hi ? "text-accent font-medium" : "text-slate-300"}">
        <td class="py-1 pr-2">${name}</td><td class="text-right">${m.totalRet}%</td><td class="text-right">${m.cagr}%</td>
        <td class="text-right text-bad">${m.maxDD}%</td><td class="text-right">${m.volAnn}%</td><td class="text-right">${m.sharpe ?? "—"}</td></tr>`;
    const benches = Object.entries(d.benchmarks).map(([k, v]) => row(k, v)).join("");
    el.innerHTML = `<div class="bg-panel2 border border-edge rounded-xl p-3">
      <div class="text-[11px] text-slate-400 mb-2">${t("strat_bt_period")}: ${d.period.years} ${t("strat_bt_yr")} (${d.period.months} ${t("strat_bt_mo")}) · ${d.assets.length} ${t("strat_assets")}</div>
      ${d.equity ? equityChart(d.equity) : ""}
      <table class="w-full text-xs"><thead><tr class="text-slate-500 text-[10px]"><th class="text-left font-normal">${t("strat_btn")}</th><th class="text-right font-normal">${t("bt_total")}</th><th class="text-right font-normal">CAGR</th><th class="text-right font-normal">Max DD</th><th class="text-right font-normal">${t("bt_vol")}</th><th class="text-right font-normal">Sharpe</th></tr></thead>
      <tbody>${row("🏛 " + t("prof_" + currentProfile) + " (" + t("strat_bt_static") + ")", d.portfolio)}${d.portfolioDynamic ? row(t("strat_bt_dynamic"), d.portfolioDynamic, true) : ""}${benches}</tbody></table>
      <div class="text-[10px] text-slate-500 mt-2">${t("strat_bt_note")}</div>
    </div>`;
  } catch (e) { el.innerHTML = `<div class="text-xs text-bad">${(e as Error).message}</div>`; }
}

// Спарклайн із масиву значень (для трендів історії)
export function sparkline(vals: number[], color: string, w = 90, h = 20): string {
  if (vals.length < 2) return "";
  const mx = Math.max(...vals), mn = Math.min(...vals);
  const X = (i: number): number => (i / (vals.length - 1)) * w;
  const Y = (v: number): number => h - 2 - ((v - mn) / ((mx - mn) || 1)) * (h - 4);
  const dpath = vals.map((v, i) => `${i ? "L" : "M"}${X(i).toFixed(1)} ${Y(v).toFixed(1)}`).join(" ");
  return `<svg viewBox="0 0 ${w} ${h}" width="${w}" height="${h}" class="inline-block align-middle"><path d="${dpath}" fill="none" stroke="${color}" stroke-width="1.5"/></svg>`;
}

// Netflow біржі: приток на CEX (роздача) vs відтік із CEX (накопичення)
async function loadNetflow(contract: string): Promise<void> {
  const el = document.getElementById("conc-netflow");
  if (!el) return;
  try {
    const d = await getJSON<{ inUsd: number; outUsd: number; netUsd: number; inCount: number; outCount: number; verdict: string; windowFrom: number | null; windowTo: number | null }>("/api/exchange-netflow", { contract });
    if ((d.inCount + d.outCount) < 1) { el.innerHTML = ""; return; }
    const vCol = d.verdict === "accumulation" ? "text-good" : d.verdict === "distribution" ? "text-bad" : "text-slate-400";
    const vIcon = d.verdict === "accumulation" ? "🟢" : d.verdict === "distribution" ? "🔴" : "⚪";
    const total = d.inUsd + d.outUsd || 1;
    const outPct = Math.round((d.outUsd / total) * 100);
    const sd = (ts: number): string => new Date(ts).toISOString().slice(0, 10);
    const period = d.windowFrom && d.windowTo ? `${sd(d.windowFrom)} – ${sd(d.windowTo)}` : "";
    el.innerHTML = `<div class="bg-panel2 border border-edge rounded-xl p-3">
      <div class="flex items-center justify-between mb-1.5">
        <span class="text-xs text-slate-400">🏦 ${t("nf_title")}</span>
        <span class="text-xs font-medium ${vCol}">${vIcon} ${t("nf_" + d.verdict)}</span>
      </div>
      <div class="flex h-2.5 rounded-full overflow-hidden bg-bad/40 mb-1.5"><div class="bg-good" style="width:${outPct}%"></div></div>
      <div class="flex justify-between text-[11px]">
        <span class="text-good">↙ ${t("nf_out")}: ${fmtUsd(d.outUsd)} <span class="text-slate-500">· ${d.outCount}</span></span>
        <span class="text-bad">↗ ${t("nf_in")}: ${fmtUsd(d.inUsd)} <span class="text-slate-500">· ${d.inCount}</span></span>
      </div>
      <div class="text-[10px] text-slate-500 mt-1">${t("nf_net")}: <span class="${d.netUsd >= 0 ? "text-good" : "text-bad"}">${d.netUsd >= 0 ? "+" : ""}${fmtUsd(d.netUsd)}</span>${period ? ` · ${period}` : ""} · ${t("nf_note")}</div>
    </div>`;
  } catch { el.innerHTML = ""; }
}

// Тренд концентрації токена з історичного сховища (накопичується воркером)
async function loadConcTrend(symbol?: string): Promise<void> {
  const el = document.getElementById("conc-trend");
  if (!el || !symbol) return;
  try {
    const d = await getJSON<{ series: Array<{ t: number; top10: number }> }>("/api/history", { key: `conc:${symbol}` });
    const s = (d.series || []).filter((p) => typeof p.top10 === "number");
    if (s.length < 2) { el.innerHTML = `<span class="text-[11px] text-slate-500">📉 ${t("hist_collecting")}</span>`; return; }
    const vals = s.map((p) => p.top10);
    const diff = +(vals[vals.length - 1] - vals[0]).toFixed(2);
    const up = diff >= 0; // зростання концентрації = централізація (червоне), падіння = розподіл (зелене)
    el.innerHTML = `<span class="text-[11px] text-slate-400">${t("hist_conc_trend")}: </span>${sparkline(vals, up ? "#ef4444" : "#22c55e")} <span class="text-[11px] ${up ? "text-bad" : "text-good"}">${up ? "▲" : "▼"} ${Math.abs(diff)}%</span> <span class="text-[10px] text-slate-500">(${s.length} ${t("hist_points")})</span>`;
  } catch { /* ignore */ }
}

interface AssetFlowPoint { price: number; ts: number; }
interface WhalePnl { current: number; avgEntryPct: number | null; lastEntryPct: number | null; phase: "accumulation" | "in_profit" | "distribution_risk" | "unknown"; }
interface Liquidity { volume24h: number | null; marketCap: number | null; volToMcap: number | null; tier: "deep" | "moderate" | "thin" | "unknown"; drawdownContext: boolean; }
interface PumpSignal { phase: "prepump" | "pump" | "distribution" | "neutral"; exitRatio: number; recentBuyRatio: number; socialSpike: boolean | null; galaxyScore: number | null; sentiment: number | null; socialVolume24h: number | null; socialEnabled: boolean; socialSource: string | null; }
interface LeaderRow { addr: string; share: number; avgEntry: number; currentValue: number; profitUsd: number; multiple: number | null; }
interface AssetFlow { chart: Array<{ ts: number; price: number }>; currentPrice: number; marketCap: number | null; fdv: number | null; avgEntry: AssetFlowPoint | null; avgExit: AssetFlowPoint | null; lastEntry: AssetFlowPoint | null; whalePnl?: WhalePnl; liquidity?: Liquidity; pumpSignal?: PumpSignal; leaderboard?: LeaderRow[]; holdersAnalyzed: number; txCount: number; }

// Графік ціни активу за весь період із позначеними середніми точками входу/виходу основних тримачів.
async function loadAssetFlow(contract: string): Promise<void> {
  const el = document.getElementById("conc-flow");
  if (!el) return;
  try {
    const d = await getJSON<AssetFlow>("/api/asset-flow", { contract });
    if (!d.chart?.length) { el.innerHTML = `<div class="text-[11px] text-slate-500">${t("af_none")}</div>`; return; }
    el.innerHTML = assetFlowHtml(d);
  } catch { el.innerHTML = `<div class="text-[11px] text-slate-500">${t("af_none")}</div>`; }
}

function assetFlowHtml(d: AssetFlow): string {
  const W = 520, H = 150, padL = 6, padR = 6, padT = 10, padB = 18;
  const ser = d.chart;
  const xs0 = ser[0].ts, xs1 = ser[ser.length - 1].ts;
  const prices = ser.map((p) => p.price);
  const markerP = [d.avgEntry?.price, d.avgExit?.price, d.lastEntry?.price, d.currentPrice].filter((x): x is number => !!x);
  const pMax = Math.max(...prices, ...markerP), pMin = Math.min(...prices, ...markerP);
  const X = (ts: number): number => padL + ((ts - xs0) / ((xs1 - xs0) || 1)) * (W - padL - padR);
  const Y = (p: number): number => padT + (1 - (p - pMin) / ((pMax - pMin) || 1)) * (H - padT - padB);
  const line = ser.map((p, i) => `${i ? "L" : "M"}${X(p.ts).toFixed(1)} ${Y(p.price).toFixed(1)}`).join(" ");
  const area = `${line} L${X(xs1).toFixed(1)} ${H - padB} L${X(xs0).toFixed(1)} ${H - padB} Z`;
  const marker = (m: AssetFlowPoint | null, color: string): string => {
    if (!m || !m.price) return "";
    const x = X(Math.max(xs0, Math.min(xs1, m.ts))), y = Y(m.price);
    return `<line x1="${padL}" y1="${y.toFixed(1)}" x2="${W - padR}" y2="${y.toFixed(1)}" stroke="${color}" stroke-width="1" stroke-dasharray="3 3" opacity="0.7"/>
      <circle cx="${x.toFixed(1)}" cy="${y.toFixed(1)}" r="4" fill="${color}" stroke="#0b0f1a" stroke-width="1.5"/>`;
  };
  const fmtP = (p: number): string => p >= 1 ? "$" + p.toFixed(2) : "$" + p.toPrecision(3);
  const fmtD = (ts: number): string => { const x = new Date(ts); return `${x.getUTCFullYear()}-${String(x.getUTCMonth() + 1).padStart(2, "0")}`; };
  const chip = (label: string, m: AssetFlowPoint | null, color: string): string =>
    m && m.price ? `<span class="inline-flex items-center gap-1 text-[11px]"><span class="w-2 h-2 rounded-full" style="background:${color}"></span>${label}: <b>${fmtP(m.price)}</b> <span class="text-slate-500">${fmtD(m.ts)}</span></span>` : "";
  // Бейдж PnL китів vs поточна ціна: у мінусі = фаза накопичення (сигнал розумних грошей).
  const wpnlBadge = (): string => {
    const w = d.whalePnl;
    if (!w || w.avgEntryPct == null) return "";
    const pct = w.avgEntryPct, inProfit = pct >= 0;
    const sign = inProfit ? "+" : "", pctTxt = `${sign}${(pct * 100).toFixed(0)}%`;
    const hint = w.phase === "accumulation" ? t("af_wphase_acc") : w.phase === "distribution_risk" ? t("af_wphase_dist") : t("af_wphase_prof");
    const col = inProfit ? (w.phase === "distribution_risk" ? "#f59e0b" : "#94a3b8") : "#22c55e";
    const bg = inProfit ? (w.phase === "distribution_risk" ? "rgba(245,158,11,0.12)" : "rgba(148,163,184,0.12)") : "rgba(34,197,94,0.12)";
    return `<span class="inline-flex items-center gap-1 text-[11px] px-2 py-0.5 rounded-full" style="color:${col};background:${bg}" title="${hint}">🐋 ${t("af_wpnl")} <b>${pctTxt}</b> · ${inProfit ? t("af_wprofit") : t("af_wloss")}</span>`;
  };
  // Бейдж реальної ліквідності (обсяг/добу vs mcap): здорова ~10-20%; тонка = просадка ≠ вихід китів.
  const liqBadge = (): string => {
    const l = d.liquidity;
    if (!l || l.volToMcap == null) return "";
    const col = l.tier === "deep" ? "#22c55e" : l.tier === "moderate" ? "#94a3b8" : "#f59e0b";
    const bg = l.tier === "deep" ? "rgba(34,197,94,0.12)" : l.tier === "moderate" ? "rgba(148,163,184,0.12)" : "rgba(245,158,11,0.12)";
    const pct = (l.volToMcap * 100).toFixed(1);
    return `<span class="inline-flex items-center gap-1 text-[11px] px-2 py-0.5 rounded-full" style="color:${col};background:${bg}" title="${t("af_liq")}: ${fmtUsd(l.volume24h || 0)}/24г vs ${fmtUsd(l.marketCap || 0)} mcap">💧 ${t("af_liq")}: <b>${t("af_liq_" + l.tier)}</b> (${pct}%)</span>`;
  };
  // Трикольоровий сигнал пампу (🟡 пре-памп → 🟢 памп → 🔴 розподіл).
  const pumpHtml = (): string => {
    const p = d.pumpSignal;
    if (!p || p.phase === "neutral") return "";
    const map: Record<string, [string, string]> = { prepump: ["🟡", "#eab308"], pump: ["🟢", "#22c55e"], distribution: ["🔴", "#ef4444"] };
    const [ic, col] = map[p.phase] || ["", "#94a3b8"];
    const details: string[] = [];
    if (p.exitRatio > 0) details.push(`${Math.round(p.exitRatio * 100)}% ${t("af_pump_exited")}`);
    if (p.socialEnabled && p.galaxyScore != null) details.push(`GS ${p.galaxyScore}`);
    if (p.socialEnabled && p.sentiment != null) details.push(`😊 ${Math.round(p.sentiment)}%`);
    if (p.socialSpike) details.push(t("af_pump_social"));
    return `<div class="rounded-lg px-2.5 py-1.5 mb-1.5 flex items-center gap-2 flex-wrap" style="background:${col}18;border:1px solid ${col}55">
      <span class="text-sm">${ic}</span><span class="text-[12px] font-semibold" style="color:${col}">${t("af_pump")}: ${t("af_pump_" + p.phase)}</span>
      ${details.length ? `<span class="text-[10px] text-slate-400">· ${details.join(" · ")}</span>` : ""}</div>`;
  };
  const badges = [
    (d.whalePnl && d.whalePnl.avgEntryPct != null) ? wpnlBadge() : "",
    (d.liquidity && d.liquidity.volToMcap != null) ? liqBadge() : "",
  ].filter(Boolean).join(" ");
  const ddHint = d.liquidity?.drawdownContext ? `<div class="text-[10px] mt-1" style="color:#f59e0b">⚠ ${t("af_liq_dd")}</div>` : "";
  return `
    ${gaugeDashHtml(d)}
    <div class="flex items-center justify-between mb-1">
      <span class="text-xs text-slate-400">📈 ${t("af_title")}</span>
      <span class="text-[11px] text-slate-500">${d.marketCap ? `${t("af_mcap")}: <b class="text-slate-300">${fmtUsd(d.marketCap)}</b> · ` : ""}${t("af_now")}: ${fmtP(d.currentPrice)} · ${d.holdersAnalyzed} ${t("af_holders")}</span>
    </div>
    ${pumpHtml()}
    ${badges ? `<div class="flex flex-wrap gap-1.5 mb-1.5">${badges}</div>${ddHint}` : ""}
    <svg viewBox="0 0 ${W} ${H}" class="w-full" style="height:150px" preserveAspectRatio="none">
      <defs><linearGradient id="afg" x1="0" y1="0" x2="0" y2="1"><stop offset="0%" stop-color="#3b82f6" stop-opacity="0.28"/><stop offset="100%" stop-color="#3b82f6" stop-opacity="0"/></linearGradient></defs>
      <path d="${area}" fill="url(#afg)"/><path d="${line}" fill="none" stroke="#3b82f6" stroke-width="1.5"/>
      ${marker(d.avgEntry, "#22c55e")}${marker(d.avgExit, "#ef4444")}${marker(d.lastEntry, "#06b6d4")}
    </svg>
    <div class="flex flex-wrap gap-x-3 gap-y-1 mt-1.5">
      ${chip(t("af_entry"), d.avgEntry, "#22c55e")}
      ${chip(t("af_exit"), d.avgExit, "#ef4444")}
      ${chip(t("af_last_entry"), d.lastEntry, "#06b6d4")}
    </div>
    <div class="text-[10px] text-slate-500 mt-1">${t("af_note")}</div>
    ${leaderboardHtml(d.leaderboard)}`;
}

// Півколовий SVG-циферблат (гейдж): frac 0..1, кольорова дуга + значення в центрі + підпис.
function gaugeMini(frac: number, valueText: string, label: string, color: string): string {
  const f = Math.max(0, Math.min(1, frac)), r = 30, cx = 38, cy = 36, W = 76, H = 50;
  const pt = (ff: number): [number, number] => [cx + r * Math.cos(Math.PI * (1 - ff)), cy - r * Math.sin(Math.PI * (1 - ff))];
  const [x0, y0] = pt(0), [x1, y1] = pt(1), [xv, yv] = pt(f);
  const track = `M${x0.toFixed(1)} ${y0.toFixed(1)} A${r} ${r} 0 0 1 ${x1.toFixed(1)} ${y1.toFixed(1)}`;
  const val = `M${x0.toFixed(1)} ${y0.toFixed(1)} A${r} ${r} 0 0 1 ${xv.toFixed(1)} ${yv.toFixed(1)}`;
  return `<div class="flex flex-col items-center">
    <svg viewBox="0 0 ${W} ${H}" style="width:76px;height:50px">
      <path d="${track}" fill="none" stroke="#334155" stroke-width="6" stroke-linecap="round"/>
      <path d="${val}" fill="none" stroke="${color}" stroke-width="6" stroke-linecap="round"/>
      <text x="${cx}" y="${cy - 2}" text-anchor="middle" fill="${color}" font-size="13" font-weight="700">${valueText}</text>
    </svg>
    <span class="text-[9px] text-slate-400 -mt-1">${label}</span>
  </div>`;
}
// Гейдж-дашборд токена на 1 екран (vision Oracul: циферблати замість кліків).
function gaugeDashHtml(d: AssetFlow): string {
  const g: string[] = [];
  const l = d.liquidity, w = d.whalePnl, p = d.pumpSignal;
  if (l && l.volToMcap != null) {
    const col = l.tier === "deep" ? "#22c55e" : l.tier === "moderate" ? "#94a3b8" : "#f59e0b";
    g.push(gaugeMini(Math.min(1, l.volToMcap / 0.2), `${(l.volToMcap * 100).toFixed(1)}%`, `💧 ${t("af_g_liq")}`, col));
  }
  if (w && w.avgEntryPct != null) {
    const col = w.avgEntryPct >= 0 ? (w.phase === "distribution_risk" ? "#f59e0b" : "#94a3b8") : "#22c55e";
    g.push(gaugeMini(0.5 + Math.max(-1, Math.min(1, w.avgEntryPct)) / 2, `${w.avgEntryPct >= 0 ? "+" : ""}${Math.round(w.avgEntryPct * 100)}%`, `🐋 ${t("af_g_whale")}`, col));
  }
  if (p && p.socialEnabled && p.sentiment != null) {
    const col = p.sentiment >= 60 ? "#22c55e" : p.sentiment >= 40 ? "#94a3b8" : "#ef4444";
    g.push(gaugeMini(p.sentiment / 100, `${Math.round(p.sentiment)}%`, `😊 ${t("af_g_sent")}`, col));
  }
  if (p && p.exitRatio != null) {
    const col = p.exitRatio >= 0.5 ? "#ef4444" : p.exitRatio >= 0.3 ? "#f59e0b" : "#22c55e";
    g.push(gaugeMini(p.exitRatio, `${Math.round(p.exitRatio * 100)}%`, `📤 ${t("af_g_exit")}`, col));
  }
  if (g.length < 2) return "";
  return `<div class="flex flex-wrap justify-around gap-1 mb-2 pb-2 border-b border-edge/60">${g.join("")}</div>`;
}

// ── Oracle-AI (лайт): NL-запит по токену → локалізована відповідь з ончейн-метрик ──
interface TokenAI { symbol: string; intent: string; currentPrice: number | null; marketCap: number | null; avgEntry: number | null; whalePnl: WhalePnl | null; liquidity: Liquidity | null; pumpSignal: PumpSignal | null; leaderboard: LeaderRow[]; }
function formatTokenAI(d: TokenAI): string {
  const w = d.whalePnl, l = d.liquidity, p = d.pumpSignal;
  const na = `<div class="text-slate-500">${t("ai_a_nodata")}</div>`;
  const pumpLine = () => (p && p.phase !== "neutral") ? `<div>${p.phase === "prepump" ? "🟡" : p.phase === "pump" ? "🟢" : "🔴"} ${t("af_pump")}: <b>${t("af_pump_" + p.phase)}</b></div>` : "";
  const whaleLine = () => (w && w.avgEntryPct != null) ? `<div>🐋 ${w.avgEntryPct >= 0 ? t("ai_a_whales_prof") : t("ai_a_whales_loss")} (${w.avgEntryPct >= 0 ? "+" : ""}${Math.round(w.avgEntryPct * 100)}%) — ${t("af_wphase_" + (w.phase === "accumulation" ? "acc" : w.phase === "distribution_risk" ? "dist" : "prof"))}</div>` : "";
  const liqLine = () => (l && l.tier !== "unknown") ? `<div>💧 ${t("af_liq")}: <b>${t("af_liq_" + l.tier)}</b> (${l.volToMcap != null ? (l.volToMcap * 100).toFixed(1) + "%" : "—"})${l.drawdownContext ? ` · ⚠ ${t("af_liq_dd")}` : ""}</div>` : "";
  const priceLine = () => `<div>${t("ai_a_price")}: <b>${d.currentPrice ? fmtUsd(d.currentPrice) : "—"}</b>${d.avgEntry ? ` · ${t("af_entry")}: ${fmtUsd(d.avgEntry)}` : ""}${d.marketCap ? ` · ${t("af_mcap")}: ${fmtUsd(d.marketCap)}` : ""}</div>`;
  const leadersLine = () => (d.leaderboard || []).filter((r) => r.profitUsd > 0).length
    ? `<div>🏆 ${t("ai_a_leaders")}:</div>` + d.leaderboard.filter((r) => r.profitUsd > 0).slice(0, 5).map((r, i) => `<div class="text-[11px]"><span class="text-slate-500">${i + 1}.</span> <span data-wallet-addr="${escAttr(r.addr)}" class="font-mono cursor-pointer hover:text-accent">${shortAddr(r.addr)}</span> ${r.multiple ? r.multiple + "x" : ""} <span class="text-good">+${fmtUsd(r.profitUsd)}</span></div>`).join("")
    : na;
  const sentLine = () => (p && p.socialEnabled && p.sentiment != null) ? `<div>😊 ${t("af_g_sent")}: <b>${Math.round(p.sentiment)}%</b>${p.socialSpike ? ` · ${t("af_pump_social")}` : ""}</div>` : na;
  let out: string;
  switch (d.intent) {
    case "liquidity": out = liqLine() || na; break;
    case "pump": out = (pumpLine() || na) + whaleLine(); break;
    case "phase": out = (whaleLine() || na) + pumpLine(); break;
    case "entry": out = priceLine() + whaleLine(); break;
    case "leaders": out = leadersLine(); break;
    case "sentiment": out = sentLine(); break;
    default: out = priceLine() + pumpLine() + whaleLine() + liqLine(); // summary
  }
  return `<div class="space-y-0.5">${out}</div><div class="text-[9px] text-slate-600 mt-1">${t("ai_note")}</div>`;
}
export async function askTokenAI(): Promise<void> {
  const ans = document.getElementById("conc-ai-answer");
  const contract = $<HTMLInputElement>("#conc-input").value.trim();
  const q = $<HTMLInputElement>("#conc-ai-input").value.trim();
  if (!ans || !/^0x[0-9a-fA-F]{40}$/.test(contract)) return;
  ans.innerHTML = `<div class="text-slate-500">${t("ai_thinking")}</div>`;
  try {
    const d = await getJSON<TokenAI>("/api/token-ai", { contract, q });
    ans.innerHTML = formatTokenAI(d);
  } catch { ans.innerHTML = `<div class="text-bad">${t("conc_err")}</div>`; }
}

// Лідерборд прибуткових гаманців по активу (клік → портфель гаманця).
function leaderboardHtml(lb: LeaderRow[] | undefined): string {
  const rows = (lb || []).filter((r) => r.profitUsd > 0).slice(0, 8);
  if (!rows.length) return "";
  const fUsd = (n: number) => n >= 1e6 ? "$" + (n / 1e6).toFixed(1) + "M" : n >= 1e3 ? "$" + (n / 1e3).toFixed(0) + "k" : "$" + Math.round(n);
  return `<div class="mt-2 pt-2 border-t border-edge/60">
    <div class="text-[10px] text-slate-400 mb-1">🏆 ${t("af_lb")}</div>
    <div class="space-y-0.5">${rows.map((r, i) => `
      <div class="flex items-center gap-2 text-[11px]">
        <span class="text-slate-500 w-4 shrink-0">${i + 1}.</span>
        <span data-wallet-addr="${escAttr(r.addr)}" class="font-mono cursor-pointer hover:text-accent">${shortAddr(r.addr)}</span>
        ${r.multiple ? `<span class="text-[10px] px-1 rounded ${r.multiple >= 2 ? "text-good" : "text-slate-400"}">${r.multiple}x</span>` : ""}
        <span class="flex-1"></span>
        <span class="text-good">+${fUsd(r.profitUsd)}</span>
        <span class="text-slate-500">${t("af_lb_holds")} ${fUsd(r.currentValue)}</span>
      </div>`).join("")}</div></div>`;
}

async function loadAssetPrice(contract: string): Promise<void> {
  const el = document.getElementById("conc-price");
  if (!el || !contract) return;
  try {
    const d = await getJSON<{ priceUsd?: number | null; marketCap?: number | null }>("/api/cg-asset", { contract });
    const parts: string[] = [];
    if (d.priceUsd != null) {
      const p = d.priceUsd;
      parts.push(`<span class="text-slate-200 font-medium">$${p >= 1 ? p.toLocaleString(undefined, { maximumFractionDigits: 2 }) : p.toPrecision(3)}</span>`);
    }
    if (d.marketCap != null) parts.push(`${t("um_mcap")} ${fmtUsd(d.marketCap)}`);
    el.innerHTML = parts.join(" \u00b7 ");
  } catch { el.innerHTML = ""; }
}

async function loadAssetBacktest(symbol: string): Promise<void> {
  const el = document.getElementById("conc-backtest");
  if (!el || !symbol) return;
  try {
    const d = await getJSON<{ assets?: Array<{ symbol: string; events: number; resolved: number; meanExcess: number | null }> }>("/api/backtest");
    const row = (d.assets || []).find((a) => (a.symbol || "").toUpperCase() === symbol.toUpperCase());
    if (!row || !row.events) { el.innerHTML = ""; return; }
    const ex = row.meanExcess;
    const col = ex == null ? "text-slate-300" : ex >= 0 ? "text-good" : "text-bad";
    el.innerHTML = `<div class="bg-panel2 border border-edge rounded-xl p-3">
      <div class="text-xs text-slate-400 mb-1.5">🔬 ${t("cbt_title")}</div>
      <div class="flex gap-2 text-xs">
        <div class="flex-1"><div class="text-[10px] text-slate-500">${t("cbt_excess")}</div><div class="text-sm font-semibold ${col}">${ex == null ? "—" : (ex >= 0 ? "+" : "") + ex + "%"}</div></div>
        <div class="flex-1"><div class="text-[10px] text-slate-500">${t("cbt_events")}</div><div class="text-sm text-slate-200">${row.resolved}/${row.events}</div></div>
      </div>
      <div class="text-[10px] text-slate-500 mt-1">${t("cbt_note")}</div>
    </div>`;
  } catch { el.innerHTML = ""; }
}

async function loadAssetNetworks(contract: string, symbol: string): Promise<void> {
  const el = document.getElementById("conc-networks");
  if (!el) return;
  try {
    const d = await getJSON<{ networks?: Array<{ size: number; conductor: string | null; members: Array<{ addr: string; assetCount: number }>; sharedAssets: Array<{ symbol: string; contract: string | null }>; collective: Array<{ symbol: string; contract: string | null; combinedShare: number; holders: number }> }> }>("/api/coordinated-networks");
    const cl = (contract || "").toLowerCase(); const su = (symbol || "").toUpperCase();
    const has = (arr: Array<{ symbol: string; contract: string | null }>) => arr.some((a) => (a.contract || "").toLowerCase() === cl || (a.symbol || "").toUpperCase() === su);
    const nets = (d.networks || []).filter((n) => has(n.sharedAssets) || has(n.collective));
    if (!nets.length) { el.innerHTML = ""; return; }
    el.innerHTML = `<div class="bg-panel2 border border-edge rounded-xl p-3">
      <div class="text-xs text-slate-400 mb-1.5">🕸 ${t("cnet_title")} <span class="text-slate-500">· ${nets.length}</span></div>` +
      nets.slice(0, 6).map((n) => {
        const cc = n.collective.find((x) => (x.contract || "").toLowerCase() === cl || (x.symbol || "").toUpperCase() === su);
        return `<div class="rounded-lg border border-fuchsia-500/25 bg-fuchsia-500/5 p-2 mb-1.5 text-[11px]">
          <div class="flex justify-between mb-0.5"><span class="text-fuchsia-300 font-medium">${n.size} ${t("cg_members")}</span>${cc ? `<span class="text-slate-300">${cc.combinedShare}% ${t("ag_of_supply")} · ${cc.holders} 🐋</span>` : ""}</div>
          <div class="flex flex-wrap gap-1">${n.members.slice(0, 8).map((mm) => `<span data-wallet-addr="${escAttr(mm.addr)}" class="font-mono px-1.5 py-0.5 rounded bg-panel border border-edge text-slate-400 cursor-pointer hover:text-accent">${shortAddr(mm.addr)}</span>`).join("")}</div>
        </div>`;
      }).join("") + `</div>`;
  } catch { el.innerHTML = ""; }
}

function renderConcentration(d: ConcData): void {
  const rows = d.holders.map((h) => {
    const cm = CLASS_META[h.cls || "holder"] || CLASS_META.holder;
    const clsBadge = `<span class="px-1.5 py-0.5 rounded text-[10px] ${cm.cls} shrink-0">${cm.icon} ${h.label || t(cm.key)}</span>`;
    return `
    <div class="py-1.5 border-b border-edge/60">
      <div class="flex items-center gap-2 text-sm">
        <span class="text-slate-500 w-7 shrink-0">#${h.rank}</span>
        ${clsBadge}
        <span data-wallet-addr="${escAttr(h.address)}" class="font-mono truncate flex-1 cursor-pointer hover:text-accent">${shortAddr(h.address)}</span>
        <span class="text-slate-300 shrink-0">${h.share}%</span>
        <span class="text-slate-500 shrink-0">Σ ${h.cum}%</span>
      </div>
      <div class="h-1 mt-1 rounded bg-panel2"><div class="h-1 rounded bg-accent" style="width:${Math.min(100, h.cum)}%"></div></div>
      <div class="flex gap-3 mt-1 text-[11px]">
        <button data-assets="${h.address}" class="text-accent hover:underline">${t("conc_assets")}</button>
        <button data-track-addr="${h.address}" data-track-sym="${escAttr(d.token.symbol)}" class="text-purple-400 hover:underline">${t("conc_track")}</button>
        <a href="${arkhamLink(h.address)}" target="_blank" rel="noopener" class="text-slate-400 hover:underline">Arkham →</a>
      </div>
      <div id="assets-${h.address}" class="hidden mt-1 pl-7 text-[11px] text-slate-400 space-y-0.5"></div>
    </div>`;
  }).join("");
  const metrics = d.scores ? `
    <div class="bg-panel2 border border-edge rounded-xl p-3">
      <div class="grid grid-cols-3 gap-2">
        ${gaugeSvg(d.scores.entry, t("intel_entry"))}
        ${gaugeSvg(d.scores.fearGreed, t("intel_fg"))}
        ${gaugeSvg(d.scores.behaviour, t("intel_behaviour"))}
      </div>
      <div class="text-[10px] text-slate-500 mt-2">${t("intel_disclaimer")}</div>
    </div>` : "";
  const ORDER = ["cex", "mm", "fund", "whale", "contract", "retail", "burn"];
  const bd = d.breakdown || {};
  const who = Object.keys(bd).length ? `<div class="bg-panel2 border border-edge rounded-xl p-3">
      <div class="text-xs text-slate-400 mb-2">${t("conc_who_holds")}</div>
      <div class="flex flex-wrap gap-1.5">${ORDER.filter((k) => bd[k]).map((k) => { const m = CLASS_META[k] || CLASS_META.holder; return `<span class="px-2 py-1 rounded-lg text-[11px] ${m.cls}">${m.icon} ${t(m.key)} <b>${bd[k].sharePct}%</b> · ${bd[k].count}</span>`; }).join("")}</div>
    </div>` : "";
  const uni = universeByContract(d.token.contract);
  const uniMeta = uni
    ? `<div class="flex items-center gap-2 mt-1 text-xs">${uni.rank ? `<span class="text-slate-400">${t("um_rank")} <span class="text-slate-200 font-medium">#${uni.rank}</span></span>` : ""}<span class="px-1.5 py-0.5 rounded bg-panel border border-edge text-[11px]">${sectorEmoji(uni.sector)} ${t("sec_" + uni.sector)}</span><span class="font-mono text-[10px] text-slate-500 truncate">${d.token.contract}</span></div>`
    : "";
  $("#conc-body").innerHTML = `
    <div class="bg-panel2 border border-edge rounded-xl p-3">
      <div class="font-semibold">${d.token.name} <span class="text-slate-400">${d.token.symbol}</span>${clarityBadge(d.token.symbol)}</div>
      ${uniMeta}
      <div id="conc-price" class="text-xs text-slate-400 mt-0.5"></div>
      <div class="text-xs text-slate-400 mt-0.5">${t("intel_top10")}: <span class="text-slate-200 font-medium">${d.scores?.top10 ?? "—"}%</span> · ${t("conc_top50")}: <span class="text-slate-200">${d.topCumNote}%</span> · ${Number(d.token.holdersCount).toLocaleString(localeOf())} holders</div>
      <div class="text-xs mt-1">${t("conc_reach")}: 40% → ${d.reach.p40 ? "#" + d.reach.p40 : "—"} · 60% → ${d.reach.p60 ? "#" + d.reach.p60 : "—"} · 70% → ${d.reach.p70 ? "#" + d.reach.p70 : "—"}</div>
      <div id="conc-trend" class="mt-1.5"></div>
    </div>
    <div id="conc-flow" class="bg-panel2 border border-edge rounded-xl p-3"><div class="text-[11px] text-slate-500">${t("af_loading")}</div></div>
    <div class="bg-panel2 border border-edge rounded-xl p-3">
      <div class="text-xs text-slate-400 mb-1.5">🔮 ${t("ai_title")}</div>
      <div class="flex gap-1.5">
        <input id="conc-ai-input" placeholder="${escAttr(t("ai_placeholder"))}" class="flex-1 bg-panel border border-edge rounded-lg px-2 py-1 text-sm"/>
        <button id="conc-ai-ask" class="bg-accent/15 hover:bg-accent/25 text-accent border border-accent/30 px-3 rounded-lg text-sm">${t("ai_ask")}</button>
      </div>
      <div class="mt-1.5 flex flex-wrap gap-1">
        <button class="conc-ai-q text-[10px] px-1.5 py-0.5 rounded bg-panel border border-edge hover:border-accent" data-q="${escAttr(t("ai_q_phase"))}">${t("ai_q_phase")}</button>
        <button class="conc-ai-q text-[10px] px-1.5 py-0.5 rounded bg-panel border border-edge hover:border-accent" data-q="${escAttr(t("ai_q_liq"))}">${t("ai_q_liq")}</button>
        <button class="conc-ai-q text-[10px] px-1.5 py-0.5 rounded bg-panel border border-edge hover:border-accent" data-q="${escAttr(t("ai_q_pump"))}">${t("ai_q_pump")}</button>
        <button class="conc-ai-q text-[10px] px-1.5 py-0.5 rounded bg-panel border border-edge hover:border-accent" data-q="${escAttr(t("ai_q_leaders"))}">${t("ai_q_leaders")}</button>
      </div>
      <div id="conc-ai-answer" class="mt-2 text-[12px] text-slate-300"></div>
    </div>
    <div id="conc-strategy"></div>
    <div id="conc-netflow"></div>
    <div id="conc-backtest"></div>
    <div id="conc-networks"></div>
    ${who}
    ${metrics}
    <button id="conc-groups" class="w-full bg-accent/15 hover:bg-accent/25 text-accent border border-accent/30 text-sm py-1.5 rounded-lg transition">🏦 ${t("ag_btn")}</button>
    <div id="conc-groups-result"></div>
    <div class="grid grid-cols-2 gap-2">
      <button id="conc-export" class="bg-panel2 hover:border-accent border border-edge text-sm py-1.5 rounded-lg transition">${t("conc_export")}</button>
      <button id="conc-export-csv" class="bg-panel2 hover:border-accent border border-edge text-sm py-1.5 rounded-lg transition">⬇ ${t("conc_export_csv")}</button>
    </div>
    <div>${rows}</div>`;
}

function renderAssetGroups(d: { analyzed: number; totalBigShare: number; groups: Array<{ size: number; cumShare: number; members: Array<{ address: string; share: number; label: string | null }> }>; flags: { reach40: boolean; reach60: boolean; reach70: boolean } }): string {
  const largest = d.groups[0];
  const flag = d.flags.reach70 ? "≥70%" : d.flags.reach60 ? "≥60%" : d.flags.reach40 ? "≥40%" : null;
  const head = `<div class="rounded-lg border ${flag ? "border-bad/40 bg-bad/10" : "border-edge bg-panel2"} p-2 mb-2 text-xs">
      ${t("ag_top")} ${d.analyzed} · ${t("ag_sum")}: ${d.totalBigShare}% · ${t("ag_largest")}: ${largest?.cumShare || 0}%${flag ? ` <span class="text-bad font-semibold">⚠ ${flag} ${t("ag_warn")}</span>` : ""}</div>`;
  const multi = d.groups.filter((g) => g.size > 1);
  const cards = (multi.length ? multi : d.groups).slice(0, 8).map((g, i) => `<div class="rounded-lg border border-edge bg-panel2 p-2 mb-1.5">
      <div class="flex justify-between text-xs mb-1"><span class="font-medium">🏦 ${t("ag_group")} ${i + 1} · ${g.size}</span><span class="text-accent font-semibold">${g.cumShare}% ${t("ag_of_supply")}</span></div>
      ${g.members.slice(0, 6).map((m) => `<div class="flex justify-between text-[11px] text-slate-400"><span data-wallet-addr="${escAttr(m.address)}" class="font-mono truncate max-w-[62%] cursor-pointer hover:text-accent">${m.label || shortAddr(m.address)}</span><span>${m.share}%</span></div>`).join("")}
    </div>`).join("");
  return head + (multi.length ? "" : `<div class="text-[11px] text-slate-500 mb-1">${t("ag_no_groups")}</div>`) + cards;
}

export async function loadAssetGroups(): Promise<void> {
  const contract = $<HTMLInputElement>("#conc-input").value.trim();
  const out = $("#conc-groups-result");
  out.innerHTML = `<div class="text-xs text-slate-500 py-2">${t("ag_loading")}</div>`;
  try {
    const d = await getJSON<{ analyzed: number; totalBigShare: number; groups: Array<{ size: number; cumShare: number; members: Array<{ address: string; share: number; label: string | null }> }>; flags: { reach40: boolean; reach60: boolean; reach70: boolean } }>("/api/asset-groups", { contract });
    out.innerHTML = renderAssetGroups(d);
  } catch { out.innerHTML = `<div class="text-bad text-xs py-2">${t("conc_err")}</div>`; }
}

export async function loadHolderAssets(addr: string): Promise<void> {
  const el = document.querySelector<HTMLElement>(`#assets-${addr}`);
  if (!el) return;
  el.classList.toggle("hidden");
  if (el.dataset.loaded) return;
  el.textContent = t("conc_loading");
  try {
    const d = await getJSON<{ assets?: Array<{ symbol: string; valueUsd: number }> }>("/api/holder-assets", { address: addr });
    concAssets[addr] = d.assets || [];
    el.dataset.loaded = "1";
    el.innerHTML = (d.assets || []).slice(0, 8).map((a: { symbol: string; valueUsd: number }) => `<div class="flex justify-between gap-2"><span>${a.symbol}</span><span>${fmtUsd(a.valueUsd)}</span></div>`).join("") || "—";
  } catch (e) { el.textContent = `${t("conc_err")}: ${(e as Error).message}`; }
}

export function exportConcentration(): void {
  if (!lastConc) return;
  const data = { source: "Wallet Radar — public Ethplorer data", token: lastConc.token, scores: lastConc.scores, reach: lastConc.reach, top50CombinedPct: lastConc.topCumNote, holders: lastConc.holders, holderAssets: concAssets };
  const blob = new Blob([JSON.stringify(data, null, 2)], { type: "application/json" });
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url; a.download = `whales-${lastConc.token.symbol || "token"}.json`; a.click();
  URL.revokeObjectURL(url);
  toast("⬇ " + t("conc_export"), lastConc.token.symbol, "border-good");
}

export function exportConcentrationCsv(): void {
  if (!lastConc) return;
  const csv = ["rank,address,share_pct,cumulative_pct,class,label"]
    .concat(lastConc.holders.map((h) => `${h.rank},${h.address},${h.share},${h.cum},${h.cls || ""},${(h.label || "").replace(/[,\n]/g, " ")}`))
    .join("\n");
  const blob = new Blob([csv], { type: "text/csv;charset=utf-8" });
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url; a.download = `holders-${lastConc.token.symbol || "token"}.csv`; a.click();
  URL.revokeObjectURL(url);
  toast("⬇ CSV", `${lastConc.token.symbol} · ${lastConc.holders.length}`, "border-good");
}

