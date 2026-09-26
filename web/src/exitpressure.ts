/* ---------- Тиск на вихід: позиція в днях обороту, а не в доларах ----------

   Питання екрана: хто тримає стільки, що вийти без просідання не зможе, — і чи він
   уже почав виходити. Розмір у доларах на це не відповідає: $10 млн у ліквідному
   токені продаються за годину, $10 млн у тонкому — місяцями.

   Жоден інструмент ринку не зводить розмір позиції з добовим оборотом і фазою. Тут
   ці три числа стоять поруч, і кожне з них видно окремо — оцінка не замінює їх, а
   лише впорядковує. */

import { t } from "./i18n";
import { getJSON } from "./api";
import { fmtUsd } from "./util";
import { escAttr } from "./ui";

interface Row {
  group: string; groupName: string; symbol: string; contract: string;
  emissionPct: number | null; usd: number | null; daysOfVolume: number; vol24: number | null;
  phase: string; distributing: boolean; net90dUsd: number | null; exitableUsd: number | null;
  signal: string; pressure: number;
}
interface NoMarketRow { group: string; symbol: string; contract: string; usd: number | null; vol24: number | null; emissionPct: number | null; phase: string }
interface Resp {
  asOf: number; groupsWithSignals: number; groupsTotal: number;
  assets: Row[]; noMarket: NoMarketRow[]; trappedAndSelling: number;
  dropped: { unpriceable: number; insaneUsd: number; noEmission: number; noVolume: number; dust: number };
  method: string;
}

/** Дні обороту — головне число екрана, тому воно ще й словами. */
function daysWord(d: number): string {
  if (d >= 365) return `${(d / 365).toFixed(1)} ${t("ep_years")}`;
  if (d >= 30) return `${(d / 30).toFixed(1)} ${t("ep_months")}`;
  return `${d.toFixed(1)} ${t("ep_days")}`;
}

function pressCls(p: number): string {
  if (p >= 80) return "bg-bad/15 text-bad border-bad/40";
  if (p >= 55) return "bg-amber-500/15 text-amber-400 border-amber-500/40";
  return "bg-panel2 text-slate-300 border-edge";
}

function rowHtml(r: Row): string {
  return `<tr class="border-t border-edge/60">
      <td class="py-1.5 pr-2"><span class="px-1.5 py-0.5 rounded border text-[11px] font-medium ${pressCls(r.pressure)}">${r.pressure.toFixed(0)}</span></td>
      <td class="py-1.5 pr-2">
        <span class="font-medium cursor-pointer hover:text-accent" data-ep-contract="${escAttr(r.contract)}">${escAttr(r.symbol)}</span>
        <!-- Посилання на структурну мапу групи. Сторінка group.html існує давно й досі
             не мала ЖОДНОГО входу: потрапити на неї можна було, лише вписавши адресу
             руками. Тут вона нарешті доступна звідти, де про групу й читають. -->
        <div class="text-[10px]"><a href="group.html?id=${encodeURIComponent(r.group)}" target="_blank" rel="noopener" class="text-slate-500 hover:text-accent hover:underline">${escAttr(r.group)} ↗</a></div>
      </td>
      <td class="py-1.5 pr-2 text-right tabular-nums">${r.emissionPct != null ? r.emissionPct.toFixed(2) + "%" : "—"}</td>
      <td class="py-1.5 pr-2 text-right tabular-nums">${r.usd != null ? fmtUsd(r.usd) : "—"}</td>
      <td class="py-1.5 pr-2 text-right tabular-nums ${r.daysOfVolume >= 30 ? "text-bad" : r.daysOfVolume >= 7 ? "text-amber-400" : "text-slate-300"}">${daysWord(r.daysOfVolume)}</td>
      <td class="py-1.5 text-[11px] ${r.distributing ? "text-bad" : "text-slate-400"}">${escAttr(r.phase)}</td>
    </tr>`;
}

function render(d: Resp): string {
  if (!d.assets.length && !d.noMarket.length) {
    return `<div class="text-slate-400 text-sm py-8 text-center">${t("ep_empty")}</div>`;
  }
  const trapped = d.assets.filter((a) => a.distributing && a.daysOfVolume >= 7);

  const head = `
    <div class="rounded-xl border border-accent/30 bg-accent/10 p-4 mb-3">
      <div class="text-xs text-slate-400 mb-1">${t("ep_head")}</div>
      <div class="flex flex-wrap gap-x-4 gap-y-1 text-[12px]">
        <span>${t("ep_ranked")}: <span class="text-slate-100 font-medium">${d.assets.length}</span></span>
        <span class="text-bad">${t("ep_trapped")}: <span class="font-medium">${d.trappedAndSelling}</span></span>
        <span>${t("ep_nomarket")}: <span class="text-amber-400">${d.noMarket.length}</span></span>
        <span class="text-slate-500">${t("ep_groups", { n: String(d.groupsWithSignals), all: String(d.groupsTotal) })}</span>
      </div>
    </div>`;

  const trappedBlock = trapped.length ? `
    <div class="rounded-xl border border-bad/30 bg-bad/5 p-3 mb-3">
      <div class="text-xs font-medium text-bad mb-1.5">${t("ep_trapped_h")}</div>
      <div class="space-y-1">${trapped.slice(0, 8).map((r) => `
        <div class="flex justify-between gap-2 text-[12px]">
          <span class="cursor-pointer hover:text-accent" data-ep-contract="${escAttr(r.contract)}">${escAttr(r.symbol)} <span class="text-slate-500">${escAttr(r.group)}</span></span>
          <span class="text-slate-400">${r.usd != null ? fmtUsd(r.usd) : "—"} · ${daysWord(r.daysOfVolume)}</span>
        </div>`).join("")}</div>
    </div>` : "";

  const table = `
    <div class="overflow-x-auto"><table class="w-full text-[12px]">
      <thead><tr class="text-[10px] uppercase tracking-wide text-slate-500 text-left">
        <th class="pb-1 font-normal">${t("ep_col_pressure")}</th>
        <th class="pb-1 font-normal">${t("ep_col_asset")}</th>
        <th class="pb-1 font-normal text-right">${t("ep_col_emission")}</th>
        <th class="pb-1 font-normal text-right">${t("ep_col_size")}</th>
        <th class="pb-1 font-normal text-right">${t("ep_col_days")}</th>
        <th class="pb-1 font-normal">${t("ep_col_phase")}</th>
      </tr></thead><tbody>${d.assets.slice(0, 40).map(rowHtml).join("")}</tbody>
    </table></div>`;

  const nm = d.noMarket.length ? `
    <div class="rounded-xl border border-edge bg-panel2 p-3 mt-3">
      <div class="text-xs font-medium mb-1">${t("ep_nomarket_h")} (${d.noMarket.length})</div>
      <div class="text-[10px] text-slate-500 mb-1.5">${t("ep_nomarket_note")}</div>
      <div class="text-[11px] text-slate-400">${d.noMarket.slice(0, 20).map((r) => escAttr(r.symbol)).join(" · ")}</div>
    </div>` : "";

  const dropped = Object.entries(d.dropped).filter(([, v]) => v > 0)
    .map(([k, v]) => `${t("ep_drop_" + k)}: ${v}`).join(" · ");

  return head + trappedBlock + table + nm + `
    <div class="text-[10px] text-slate-500 mt-3">${escAttr(d.method)}</div>
    ${dropped ? `<div class="text-[10px] text-slate-500 mt-1">${t("ep_dropped")}: ${dropped}</div>` : ""}`;
}

export async function openExitPressure(): Promise<void> {
  let modal = document.getElementById("ep-modal");
  if (!modal) {
    modal = document.createElement("div");
    modal.id = "ep-modal";
    modal.className = "fixed inset-0 z-50 flex items-center justify-center p-4";
    modal.innerHTML = `<div id="ep-backdrop" class="absolute inset-0 bg-black/70"></div>
      <div class="relative bg-panel border border-edge rounded-2xl w-full max-w-3xl max-h-[88vh] overflow-y-auto p-5 shadow-2xl">
        <div class="flex items-center justify-between mb-3">
          <h2 class="text-base font-semibold">🚪 <span id="ep-title"></span></h2>
          <button id="ep-close" class="text-slate-400 hover:text-white text-xl leading-none">✕</button>
        </div>
        <div id="ep-body"></div>
      </div>`;
    document.body.appendChild(modal);
    modal.querySelector("#ep-backdrop")!.addEventListener("click", closeExitPressure);
    modal.querySelector("#ep-close")!.addEventListener("click", closeExitPressure);
  }
  modal.classList.remove("hidden");
  (modal.querySelector("#ep-title") as HTMLElement).textContent = t("ep_title");
  const body = modal.querySelector("#ep-body") as HTMLElement;
  body.innerHTML = `<div class="text-slate-400 text-sm py-8 text-center">${t("ep_loading")}</div>`;
  try {
    body.innerHTML = render(await getJSON<Resp>("/api/exit-pressure"));
  } catch {
    body.innerHTML = `<div class="text-bad text-sm py-8 text-center">${t("ep_err")}</div>`;
  }
}

export function closeExitPressure(): void {
  document.getElementById("ep-modal")?.classList.add("hidden");
}
