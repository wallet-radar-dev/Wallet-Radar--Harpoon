/* ---------- Вік даних на екрані ----------

   Найчастіша претензія до інструментів цього ринку — не помилки в числах, а те, що
   свіжа на вигляд цифра насправді тримісячна. Рецензент Nansen окремо вказував на
   затримку до 15 хвилин без жодної позначки; у нас стан гірший: частина панелей
   живиться файлами, чиї воркери вимкнені, і віддає їх із кодом 200, ніби все гаразд.

   Цей індикатор не лагодить воркери — він робить їхній стан видимим. Порожній
   показник тут був би найгіршим варіантом: якщо `/api/workers-health` не відповів,
   так і написано, а не «все добре». */

import { t } from "./i18n";
import { getJSON } from "./api";

interface WorkerRow {
  key: string; label: string; exists: boolean;
  processed: number | null; total: number | null; running: boolean | null;
  ageMin: number; ok: boolean;
}
interface Health { workers: WorkerRow[]; healthy: number; total: number }

function ageText(min: number): string {
  if (min < 0) return t("fr_never");
  if (min < 60) return `${min} ${t("fr_min")}`;
  if (min < 1440) return `${Math.round(min / 60)} ${t("fr_hour")}`;
  return `${Math.round(min / 1440)} ${t("fr_day")}`;
}

interface AgeRow {
  file: string; what: string; exists: boolean; bytes: number;
  ageSec: number; ttlHours: number; stale: boolean; suspiciouslyEmpty: boolean;
}
interface Ages { asOf: number; total: number; stale: number; missing: number; items: AgeRow[]; note: string }

/** Набори даних: те, чого воркер-індикатор не показує. Воркерів сім, а файлів,
    на яких тримаються панелі, — понад двадцять, і частина з них не оновлюється
    місяцями, бо їхні воркери вимкнені. */
function agesHtml(a: Ages): string {
  const bad = a.items.filter((r) => r.stale || r.suspiciouslyEmpty || !r.exists);
  if (!bad.length) return `<div class="text-[11px] text-good mt-2">${t("fr_data_ok", { n: String(a.total) })}</div>`;
  const rows = bad.slice(0, 12).map((r) => {
    const why = !r.exists ? t("fr_missing")
      : r.suspiciouslyEmpty ? t("fr_empty")
      : `${Math.round(r.ageSec / 86400)} ${t("fr_day_short")}`;
    return `<div class="flex items-center justify-between gap-3 py-0.5">
        <span class="truncate text-slate-300">${r.what}</span>
        <span class="shrink-0 text-bad text-[11px]">${why}</span>
      </div>`;
  }).join("");
  return `<div class="mt-2 pt-2 border-t border-edge/60">
      <div class="text-[11px] text-slate-400 mb-1">${t("fr_data_h", { n: String(bad.length), all: String(a.total) })}</div>
      <div class="text-[12px]">${rows}</div>
    </div>`;
}

function panelHtml(h: Health): string {
  const rows = h.workers.slice()
    .sort((a, b) => Number(a.ok) - Number(b.ok) || b.ageMin - a.ageMin)
    .map((w) => {
      const dot = w.ok ? "bg-good" : "bg-bad";
      const prog = w.total && w.processed != null ? ` · ${w.processed}/${w.total}` : "";
      const run = w.running ? ` · ${t("fr_running")}` : "";
      return `<div class="flex items-center justify-between gap-3 py-1 border-b border-edge/40 last:border-0">
          <span class="flex items-center gap-2 min-w-0"><span class="w-1.5 h-1.5 rounded-full shrink-0 ${dot}"></span><span class="truncate">${w.label}</span></span>
          <span class="text-[11px] shrink-0 ${w.ok ? "text-slate-400" : "text-bad"}">${ageText(w.ageMin)}${prog}${run}</span>
        </div>`;
    }).join("");
  const stale = h.total - h.healthy;
  return `<div class="text-[11px] text-slate-400 mb-1.5">${stale ? t("fr_stale_note", { n: String(stale) }) : t("fr_all_fresh")}</div>
    <div class="text-[12px]">${rows}</div>
    <div class="text-[10px] text-slate-500 mt-2">${t("fr_note")}</div>`;
}

let panel: HTMLElement | null = null;

function togglePanel(h: Health | null, a: Ages | null): void {
  if (panel) { panel.remove(); panel = null; return }
  const anchor = document.getElementById("freshness");
  if (!anchor) return;
  panel = document.createElement("div");
  panel.className = "absolute right-0 top-full mt-1 w-80 bg-panel border border-edge rounded-xl p-3 shadow-2xl z-50 max-h-[70vh] overflow-y-auto";
  panel.innerHTML = (h ? panelHtml(h) : `<div class="text-[12px] text-bad">${t("fr_err")}</div>`)
    + (a ? agesHtml(a) : "");
  anchor.appendChild(panel);
}

export async function initFreshness(): Promise<void> {
  const anchor = document.getElementById("freshness");
  if (!anchor) return;
  anchor.classList.add("relative");

  const paint = async (): Promise<void> => {
    let h: Health | null = null;
    try { h = await getJSON<Health>("/api/workers-health"); } catch { h = null }
    const badge = anchor.querySelector("#fr-badge") as HTMLElement | null;
    const el = badge ?? document.createElement("button");
    if (!badge) {
      el.id = "fr-badge";
      el.className = "text-xs px-2 py-1 rounded-lg border transition whitespace-nowrap";
      el.addEventListener("click", (e) => { e.stopPropagation(); void (async () => {
        // Обидва джерела паралельно: воркери й набори даних. Якщо одне впало,
        // показуємо друге, а не порожню панель.
        const [cur, ages] = await Promise.all([
          getJSON<Health>("/api/workers-health").catch(() => null),
          getJSON<Ages>("/api/data-ages").catch(() => null),
        ]);
        togglePanel(cur, ages);
      })(); });
      anchor.appendChild(el);
      document.addEventListener("click", () => { if (panel) { panel.remove(); panel = null } });
    }
    if (!h) {
      // Невідомий стан — окремий вигляд, не зелений і не прихований.
      el.className = "text-xs px-2 py-1 rounded-lg border border-amber-500/40 bg-amber-500/10 text-amber-400 transition whitespace-nowrap";
      el.textContent = `⏱ ${t("fr_unknown")}`;
      return;
    }
    const stale = h.total - h.healthy;
    el.className = `text-xs px-2 py-1 rounded-lg border transition whitespace-nowrap ${stale
      ? "border-bad/40 bg-bad/10 text-bad"
      : "border-good/40 bg-good/10 text-good"}`;
    el.textContent = stale ? `⏱ ${t("fr_stale", { n: String(stale), all: String(h.total) })}` : `⏱ ${t("fr_fresh")}`;
  };

  await paint();
  // Раз на 5 хвилин: частіше немає сенсу, воркери й так рахують хвилинами.
  window.setInterval(() => void paint(), 5 * 60 * 1000);
}
