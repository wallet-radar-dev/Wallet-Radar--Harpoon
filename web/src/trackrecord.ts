/* ---------- Послужний список сигналів: чи вони працювали ----------

   Навіщо цей екран. Кожен інструмент цього ринку показує сигнали й жоден не показує,
   чи вони справдились. Наш журнал фіксує кожне твердження в момент народження разом із
   ціною, а дозрівання рахує результат через 1/7/30/90 днів. Тут це стає видимим.

   ГОЛОВНЕ, ЧОГО ТУТ НЕ МОЖНА РОБИТИ, — показувати частку влучань саму по собі. Вона
   обманює: «вгадано падіння 72%» виглядає переконливо, поки поруч не стоїть 74% у
   випадкового входу в ті самі активи. Тому нульова база не ховається у виноску, а
   стоїть у тому ж рядку, і присуд рахується з різниці. */

import { t } from "./i18n";
import { getJSON } from "./api";

interface Verdict {
  kind: "insufficient" | "no-null-base" | "same-as-random" | "better" | "worse";
  need?: number;
  lift_pp?: number;
  band_pp?: number;
}
interface Cell {
  source: string; kind: string; horizon_d: number; n: number;
  hit_rate: number; median_excess: number;
  null_hit_rate: number | null; null_median_excess: number | null;
  null_coverage_pct: number; assets: number;
  top_asset: string | null; top_asset_share: number;
  verdict: Verdict;
}
interface Summary {
  signals: number; outcomes: number; pending: number; unresolved: number;
  excluded_bench: number; first_ts: number; last_ts: number;
  cells: Cell[]; sources: string[];
}
interface TrResp {
  asOf: number; minN: number; benchSymbol: string;
  summary: Summary; badLines: number;
  files: Array<{ file: string; path: string; bytes: number; mtime: number; ageSec: number }>;
  caveats: string[];
}

const pct = (v: number): string => `${v >= 0 ? "" : ""}${v.toFixed(0)}%`;
const signed = (v: number): string => `${v >= 0 ? "+" : ""}${v.toFixed(2)}%`;
const day = (ts: number): string => (ts > 0 ? new Date(ts * 1000).toISOString().slice(0, 10) : "—");

function age(sec: number): string {
  if (sec < 0) return t("tr_missing");
  if (sec < 3600) return `${Math.round(sec / 60)} ${t("tr_min")}`;
  if (sec < 86400) return `${Math.round(sec / 3600)} ${t("tr_hour")}`;
  return `${Math.round(sec / 86400)} ${t("tr_day")}`;
}

/** Колір і слово присуду. Нейтральне «те саме, що випадковий вхід» — сірим: це не
    провал і не успіх, це відсутність переваги, і виглядати воно має саме так. */
function verdictBadge(v: Verdict): string {
  const box = (cls: string, text: string): string =>
    `<span class="px-2 py-0.5 rounded text-[11px] whitespace-nowrap ${cls}">${text}</span>`;
  switch (v.kind) {
    case "insufficient":
      return box("bg-panel2 text-slate-400 border border-edge", t("tr_v_insufficient", { n: String(v.need ?? 30) }));
    case "no-null-base":
      return box("bg-panel2 text-slate-400 border border-edge", t("tr_v_nonull"));
    case "better":
      return box("bg-good/10 text-good border border-good/30", t("tr_v_better", { lift: signed(v.lift_pp ?? 0) }));
    case "worse":
      return box("bg-bad/10 text-bad border border-bad/30", t("tr_v_worse", { lift: signed(v.lift_pp ?? 0) }));
    default:
      return box("bg-panel2 text-slate-300 border border-edge",
        t("tr_v_same", { lift: signed(v.lift_pp ?? 0), band: (v.band_pp ?? 0).toFixed(0) }));
  }
}

function sourceLabel(src: string): string {
  if (src.startsWith("retro-")) return `${t("tr_src_retro")} · ${src.slice(6)}`;
  return `${t("tr_src_live")} · ${src}`;
}

function cellRow(c: Cell): string {
  const dir = c.kind === "sell" ? t("tr_dir_sell") : t("tr_dir_buy");
  const nullTxt = c.null_hit_rate == null
    ? `<span class="text-slate-600">—</span>`
    : `${pct(c.null_hit_rate)} · ${signed(c.null_median_excess ?? 0)}`;
  // Якщо один актив дає більшість вибірки, це висновок про нього, а не про сигнал.
  const conc = c.top_asset && c.top_asset_share >= 30
    ? `<div class="text-[10px] text-amber-400/80 mt-0.5">${t("tr_dominated", { sym: c.top_asset, share: c.top_asset_share.toFixed(0) })}</div>`
    : "";
  const cov = c.null_coverage_pct > 0 && c.null_coverage_pct < 95
    ? `<div class="text-[10px] text-amber-400/80 mt-0.5">${t("tr_lowcover", { pct: c.null_coverage_pct.toFixed(0) })}</div>`
    : "";
  return `<tr class="border-t border-edge/60 align-top">
      <td class="py-2 pr-2 text-slate-300">${dir}<div class="text-[10px] text-slate-500">${c.horizon_d} ${t("tr_days")}</div></td>
      <td class="py-2 pr-2 text-right tabular-nums text-slate-200">${c.n}<div class="text-[10px] text-slate-500">${c.assets} ${t("tr_assets")}</div></td>
      <td class="py-2 pr-2 text-right tabular-nums">${pct(c.hit_rate)}<div class="text-[10px] text-slate-500">${signed(c.median_excess)}</div></td>
      <td class="py-2 pr-2 text-right tabular-nums text-slate-400">${nullTxt}</td>
      <td class="py-2">${verdictBadge(c.verdict)}${conc}${cov}</td>
    </tr>`;
}

function render(d: TrResp): string {
  const s = d.summary;
  if (!s.signals) {
    return `<div class="text-slate-400 text-sm py-8 text-center">${t("tr_empty")}</div>`;
  }

  const head = `
    <div class="rounded-xl border border-accent/30 bg-accent/10 p-4 mb-3">
      <div class="text-xs text-slate-400 mb-1">${t("tr_head")}</div>
      <div class="flex flex-wrap gap-x-4 gap-y-1 text-[12px]">
        <span>${t("tr_claims")}: <span class="text-slate-100 font-medium tabular-nums">${s.signals.toLocaleString()}</span></span>
        <span>${t("tr_checked")}: <span class="text-slate-100 font-medium tabular-nums">${s.outcomes.toLocaleString()}</span></span>
        <span>${t("tr_waiting")}: <span class="text-slate-300 tabular-nums">${s.pending.toLocaleString()}</span></span>
        <span>${t("tr_since")}: <span class="text-slate-300">${day(s.first_ts)}</span></span>
      </div>
      <div class="text-[11px] text-slate-400 mt-2">${t("tr_excess_note", { bench: d.benchSymbol })}</div>
    </div>`;

  // групуємо за джерелом: живий монітор і ретро-реплей не можна змішувати в одну купку
  const bySource = new Map<string, Cell[]>();
  for (const c of s.cells) {
    const arr = bySource.get(c.source) ?? [];
    arr.push(c);
    bySource.set(c.source, arr);
  }

  const tables = [...bySource.entries()].map(([src, cells]) => `
    <div class="rounded-xl border border-edge bg-panel2 p-3 mb-3">
      <div class="text-xs font-medium mb-2">${sourceLabel(src)}</div>
      <table class="w-full text-[12px]">
        <thead><tr class="text-[10px] uppercase tracking-wide text-slate-500 text-left">
          <th class="pb-1 font-normal">${t("tr_col_dir")}</th>
          <th class="pb-1 font-normal text-right">${t("tr_col_n")}</th>
          <th class="pb-1 font-normal text-right">${t("tr_col_signal")}</th>
          <th class="pb-1 font-normal text-right">${t("tr_col_random")}</th>
          <th class="pb-1 font-normal">${t("tr_col_verdict")}</th>
        </tr></thead>
        <tbody>${cells.map(cellRow).join("")}</tbody>
      </table>
      ${src.startsWith("retro-") ? `<div class="text-[10px] text-slate-500 mt-2">${t("tr_retro_note")}</div>` : ""}
    </div>`).join("");

  const warn = [
    s.unresolved ? t("tr_unresolved", { n: String(s.unresolved) }) : "",
    s.excluded_bench ? t("tr_excluded", { n: String(s.excluded_bench), sym: d.benchSymbol }) : "",
    d.badLines ? t("tr_badlines", { n: String(d.badLines) }) : "",
  ].filter(Boolean);

  const files = d.files.map((f) => `<span class="whitespace-nowrap">${f.path ? f.path + "/" : ""}${f.file}: <span class="${f.ageSec < 0 ? "text-slate-600" : "text-slate-300"}">${age(f.ageSec)}</span></span>`).join(" · ");

  return head + tables + `
    <div class="rounded-xl border border-edge bg-panel2 p-3 mb-3">
      <div class="text-xs font-medium mb-1.5">${t("tr_howread")}</div>
      <ul class="text-[11px] text-slate-400 space-y-1 list-disc list-inside">
        ${d.caveats.map((c) => `<li>${c}</li>`).join("")}
      </ul>
    </div>
    ${warn.length ? `<div class="text-[11px] text-amber-400/90 mb-2">${warn.join(" · ")}</div>` : ""}
    <div class="text-[10px] text-slate-500">${t("tr_freshness")}: ${files}</div>`;
}

export async function openTrackRecord(): Promise<void> {
  let modal = document.getElementById("tr-modal");
  if (!modal) {
    modal = document.createElement("div");
    modal.id = "tr-modal";
    modal.className = "fixed inset-0 z-50 flex items-center justify-center p-4";
    modal.innerHTML = `<div id="tr-backdrop" class="absolute inset-0 bg-black/70"></div>
      <div class="relative bg-panel border border-edge rounded-2xl w-full max-w-2xl max-h-[88vh] overflow-y-auto p-5 shadow-2xl">
        <div class="flex items-center justify-between mb-3">
          <h2 class="text-base font-semibold">📒 <span id="tr-title"></span></h2>
          <button id="tr-close" class="text-slate-400 hover:text-white text-xl leading-none">✕</button>
        </div>
        <div id="tr-body"></div>
      </div>`;
    document.body.appendChild(modal);
    modal.querySelector("#tr-backdrop")!.addEventListener("click", closeTrackRecord);
    modal.querySelector("#tr-close")!.addEventListener("click", closeTrackRecord);
  }
  modal.classList.remove("hidden");
  (modal.querySelector("#tr-title") as HTMLElement).textContent = t("tr_title");
  const body = modal.querySelector("#tr-body") as HTMLElement;
  body.innerHTML = `<div class="text-slate-400 text-sm py-8 text-center">${t("tr_loading")}</div>`;
  try {
    body.innerHTML = render(await getJSON<TrResp>("/api/track-record"));
  } catch {
    // Порожній екран тут був би найгіршим варіантом: він читався б як «сигнали не
    // перевірялись», хоча насправді не відповів сервер.
    body.innerHTML = `<div class="text-bad text-sm py-8 text-center">${t("tr_err")}</div>`;
  }
}

export function closeTrackRecord(): void {
  document.getElementById("tr-modal")?.classList.add("hidden");
}
