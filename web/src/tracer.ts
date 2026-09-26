/* ---------- Tracer: трасування потоків коштів ----------

   ІСТОРІЯ ЦЬОГО ФАЙЛА. До 24.09.2026 він малював ВИГАДАНИЙ шлях: адреси через
   randomHex(40), назви контрагентів навмання з довідника, суми через seededRand.
   Виглядало це точно так само, як справжні екрани поруч, і жодної позначки не мало.
   При цьому справжній маршрут `/api/money-trace` (пошук у ширину до 15 колін із
   виявленням депозитів на біржі) існував і вже використовувався графом.

   Тепер тут лише те, що реально знайдено в ланцюгу. Наслідок, який треба прийняти:
   дані бідніші. API дає суму в доларах і глибину, але не дає мережу й актив для
   звичайного переказу — тож мережа й актив НЕ показуються ніде, крім виходів на біржу,
   де вони справді відомі. Порожній результат лишається порожнім: «нічого не знайдено»
   чесніше за красивий вигаданий ланцюжок. */

import { t } from "./i18n";
import { $, fmtUsd, shortAddr } from "./util";
import { state } from "./state";
import { escAttr } from "./ui";
import { getJSON } from "./api";
import { arkhamLink, ENTITY_TYPE_CLS, ENTITY_HEX, WALLET_HEX } from "./attribution";

interface TraceEdge { from: string; to: string; usd: number; depth: number }
interface CexMove { from: string; to: string; exchange: string; asset: string; amount: number; usd: number; depth: number }
interface MoneyTrace {
  start: string; depthReached: number; totalNodes: number; edges: number;
  edgesList: TraceEdge[];
  cexExits: CexMove[]; cexExitTotalUsd: number;
  cexWithdrawals: CexMove[]; cexWithdrawalTotalUsd: number;
}

let tracerWid: string | null = null;

/** Головний шлях грошей: з кожного коліна беремо найбільший за сумою перехід,
    що виходить із уже досягнутих адрес. Це не «єдиний правильний» шлях, а найважчий
    за грошима — і підпис під списком каже саме це, щоб його не читали як доказ. */
function mainPath(d: MoneyTrace): TraceEdge[] {
  const reached = new Set<string>([d.start.toLowerCase()]);
  const path: TraceEdge[] = [];
  const byDepth = new Map<number, TraceEdge[]>();
  for (const e of d.edgesList || []) {
    const arr = byDepth.get(e.depth) || [];
    arr.push(e);
    byDepth.set(e.depth, arr);
  }
  const depths = [...byDepth.keys()].sort((a, b) => a - b);
  for (const dp of depths) {
    const cand = (byDepth.get(dp) || [])
      .filter((e) => reached.has(e.from.toLowerCase()))
      .sort((a, b) => b.usd - a.usd)[0];
    if (!cand) break;
    path.push(cand);
    reached.add(cand.to.toLowerCase());
    if (path.length >= 12) break;
  }
  return path;
}

function hopRow(e: TraceEdge, cex: CexMove | undefined, last: boolean): string {
  const isCex = !!cex;
  const color = isCex ? ENTITY_HEX["cex"] : WALLET_HEX;
  const title = isCex
    ? `${escAttr(cex!.exchange)} <span class="px-1.5 py-0.5 rounded ${ENTITY_TYPE_CLS["cex"]} text-[10px]">${t("entity_cex")}</span>`
    : shortAddr(e.to);
  // Актив показуємо лише там, де він справді відомий — на виході на біржу.
  const amount = isCex && cex!.asset
    ? `${cex!.amount ? cex!.amount.toLocaleString(undefined, { maximumFractionDigits: 3 }) + " " + escAttr(cex!.asset) + " · " : ""}${fmtUsd(cex!.usd || e.usd)}`
    : fmtUsd(e.usd);
  return `
    <div class="relative pl-7">
      <div class="absolute left-[9px] top-0 ${last ? "h-4" : "bottom-0"} w-px bg-edge"></div>
      <div class="absolute left-1 top-4 w-3 h-3 rounded-full ring-2 ring-panel" style="background:${color}"></div>
      <div class="pb-4 pt-1.5">
        <div class="text-[11px] ${isCex ? "text-yellow-400" : "text-slate-400"} flex items-center gap-1.5">
          ${isCex ? "🏦 " + t("action_cex") : "➡ " + t("action_transfer")} · ${t("tracer_hop")} ${e.depth}
        </div>
        <div class="flex items-center justify-between gap-2 mt-0.5">
          <span class="font-medium truncate cursor-pointer hover:text-accent" data-wallet-addr="${escAttr(e.to)}">${title}</span>
          <span class="text-xs text-slate-400 shrink-0">${amount}</span>
        </div>
        <div class="flex gap-2">
          <button data-wallet-addr="${escAttr(e.to)}" class="text-[11px] text-accent hover:underline">📊 ${t("pf_open")}</button>
          <a href="${arkhamLink(e.to)}" target="_blank" rel="noopener" class="text-[11px] text-purple-400 hover:underline">🔍 Arkham →</a>
        </div>
      </div>
    </div>`;
}

function renderTrace(d: MoneyTrace, label: string, address: string): string {
  const source = `
    <div data-wallet-addr="${escAttr(address)}" class="flex items-center gap-3 bg-panel2 border border-edge rounded-xl p-3 cursor-pointer hover:border-accent transition">
      <span class="w-3 h-3 rounded-full shrink-0" style="background:${WALLET_HEX}"></span>
      <div class="min-w-0">
        <div class="text-[11px] text-slate-400">${t("tracer_source")}</div>
        <div class="font-medium truncate">${escAttr(label)}</div>
        <div class="font-mono text-[11px] text-slate-500 truncate">${shortAddr(address)}</div>
      </div>
    </div>`;

  const stats = `
    <div class="flex flex-wrap gap-x-3 gap-y-1 text-[11px] text-slate-400 mt-2">
      <span>${t("tracer_nodes")}: <span class="text-slate-200">${d.totalNodes}</span></span>
      <span>${t("tracer_edges")}: <span class="text-slate-200">${d.edges}</span></span>
      <span>${t("tracer_depth")}: <span class="text-slate-200">${d.depthReached}</span></span>
      ${d.cexExitTotalUsd ? `<span>${t("tracer_cexout")}: <span class="text-bad">${fmtUsd(d.cexExitTotalUsd)}</span></span>` : ""}
      ${d.cexWithdrawalTotalUsd ? `<span>${t("tracer_cexin")}: <span class="text-good">${fmtUsd(d.cexWithdrawalTotalUsd)}</span></span>` : ""}
    </div>`;

  const path = mainPath(d);
  if (!path.length) {
    // Порожньо — так і кажемо. Раніше на цьому місці малювався вигаданий ланцюг.
    return source + stats + `<div class="text-slate-400 text-sm py-6 text-center">${t("tracer_none")}</div>`;
  }

  const cexByTo = new Map<string, CexMove>();
  for (const c of d.cexExits || []) cexByTo.set(c.to.toLowerCase(), c);

  const steps = path.map((e, i) => hopRow(e, cexByTo.get(e.to.toLowerCase()), i === path.length - 1)).join("");
  return source + stats + `<div class="mt-4">${steps}</div>
    <div class="text-[11px] text-slate-500">${t("tracer_mainpath")}</div>`;
}

function populateTracerWalletSelect(): void {
  const sel = $<HTMLSelectElement>("#tracer-wallet-select");
  sel.innerHTML = state.wallets.map((w) => `<option value="${w.id}">${w.label || shortAddr(w.address)}</option>`).join("");
  if (tracerWid) sel.value = tracerWid;
}

export async function openTracer(wid?: string): Promise<void> {
  const w = wid ? state.wallets.find((x) => x.id === wid) : state.wallets[0];
  if (!w) return;
  tracerWid = w.id;
  populateTracerWalletSelect();
  const body = $("#tracer-body");
  const modal = $("#tracer-modal");
  modal.classList.remove("hidden"); modal.classList.add("flex");
  body.innerHTML = `<div class="text-slate-400 text-sm py-8 text-center">${t("mt_loading")}</div>`;
  try {
    const d = await getJSON<MoneyTrace>("/api/money-trace", { address: w.address, depth: 15 });
    body.innerHTML = renderTrace(d, w.label || shortAddr(w.address), w.address);
  } catch {
    body.innerHTML = `<div class="text-bad text-sm py-8 text-center">${t("tracer_err")}</div>`;
  }
}

export function closeTracer(): void {
  tracerWid = null;
  const modal = $("#tracer-modal");
  modal.classList.add("hidden"); modal.classList.remove("flex");
}

/** Перемалювати трасування під поточний гаманець (напр., після зміни мови). */
export function refreshTracer(): void {
  if (tracerWid) void openTracer(tracerWid);
}
