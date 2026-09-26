/* ---------- Перевірка за 30 секунд (/api/check) ----------

   Цей маршрут існував у сервері з серпня й не мав жодної вітрини: 506 рядків, які
   відповідають на найчастіше питання користувача — «я бачу цю адресу вперше, чи мати
   з нею справу» — лежали мертвими. Тут вони вбудовані рівно в те місце, де питання й
   виникає: у пошуковий рядок.

   ПРАВИЛО, ЯКЕ НЕ МОЖНА ЗЛАМАТИ ОФОРМЛЕННЯМ: відсутність даних — це не «чисто».
   Сервер для невідомої адреси віддає окремий стан і coverage.known = false. Тут він
   мусить виглядати НЕ зеленим і не сірим-непомітним, а явним «ми не знаємо»: найлегший
   спосіб зробити цю функцію шкідливою — намалювати заспокійливу порожнечу. */

import { t } from "./i18n";
import { getJSON } from "./api";
import { escAttr } from "./ui";

export interface CheckFact { key: string; text: string; value?: unknown }
export interface CheckResp {
  query: string;
  kind: "token" | "wallet" | "unknown";
  grade: string;
  score: number | null;
  verdict: string;
  facts: CheckFact[];
  flags: string[];
  coverage: { known: boolean; sources: string[]; missing: string[] };
}

/** Колір літери. Невідоме («?» або порожньо) навмисне отримує попереджувальний
    відтінок, а не нейтральний: сіре читається як «нічого страшного». */
function gradeCls(grade: string, known: boolean): string {
  if (!known || !grade || grade === "?" || grade === "—") return "bg-amber-500/15 text-amber-400 border-amber-500/40";
  if (grade === "A" || grade === "B") return "bg-good/15 text-good border-good/40";
  if (grade === "C" || grade === "D") return "bg-amber-500/15 text-amber-400 border-amber-500/40";
  return "bg-bad/15 text-bad border-bad/40";
}

export function renderCheckStrip(d: CheckResp): string {
  const known = d.coverage?.known !== false;
  const g = !known ? "?" : (d.grade || "?");
  const facts = (d.facts || []).slice(0, 3)
    .map((f) => `<li class="text-[11px] text-slate-400">• ${escAttr(f.text)}</li>`).join("");
  const flags = (d.flags || []).length
    ? `<div class="flex flex-wrap gap-1 mt-1.5">${d.flags.slice(0, 4).map((f) => `<span class="text-[10px] px-1.5 py-0.5 rounded bg-bad/10 text-bad border border-bad/30">${escAttr(f)}</span>`).join("")}</div>`
    : "";
  // Джерела показуємо завжди: користувач має бачити, НА ЧОМУ ґрунтується літера,
  // а не лише саму літеру.
  const src = d.coverage?.sources?.length
    ? `<div class="text-[10px] text-slate-500 mt-1.5">${t("chk_sources")}: ${d.coverage.sources.join(", ")}</div>`
    : `<div class="text-[10px] text-amber-400/80 mt-1.5">${t("chk_nosources")}</div>`;

  return `<div class="px-4 py-3 border-b border-edge bg-panel2/60">
      <div class="flex items-start gap-3">
        <span class="shrink-0 w-9 h-9 grid place-items-center rounded-lg border text-lg font-bold ${gradeCls(g, known)}">${escAttr(g)}</span>
        <div class="min-w-0">
          <div class="text-[11px] uppercase tracking-wide text-slate-500">${t("chk_title")} · ${t(d.kind === "token" ? "chk_kind_token" : d.kind === "wallet" ? "chk_kind_wallet" : "chk_kind_unknown")}</div>
          <div class="text-[12px] text-slate-200 mt-0.5">${escAttr(d.verdict || t("chk_unknown"))}</div>
          ${facts ? `<ul class="mt-1 space-y-0.5">${facts}</ul>` : ""}
          ${flags}
          ${src}
        </div>
      </div>
    </div>`;
}

/** Адреса EVM — єдиний вхід, який сервер приймає без здогадок. Для тикера він свідомо
    віддає 400: підставляти контракт за тикером небезпечно, бо однакових тикерів у наших
    даних вистачає, і колись це вже підставляло ціну від чужого токена. */
export const isEvmAddress = (s: string): boolean => /^0x[0-9a-fA-F]{40}$/.test(s.trim());

export async function fetchCheck(q: string): Promise<CheckResp | null> {
  try { return await getJSON<CheckResp>("/api/check", { q }); }
  catch { return null; }
}

/* ---------- Масова перевірка списком ----------

   Чого немає навіть у промислових розслідувальних інструментах: у відгуках на
   Chainalysis Reactor масове завантаження адрес — стала претензія. Аналітик має
   список (контрагенти угоди, холдери токена, вивантаження з біржі) і мусить
   перевіряти по одній.

   ПРАВИЛО ЗВЕДЕННЯ: невідомі адреси стоять ОКРЕМИМ числом і не входять у розподіл
   літер. Список із 90 невідомих і 10 чистих не має виглядати як «переважно чисто». */

interface BulkResp {
  checked: number; limit: number; truncated: number; invalid: string[];
  summary: { byGrade: Record<string, number>; unknown: number; withFlags: number };
  results: CheckResp[];
}

const GRADE_ORDER = ["F", "E", "D", "C", "B", "A", "—", "?"];

function bulkRow(r: CheckResp): string {
  const known = r.coverage?.known !== false;
  const g = !known ? "?" : (r.grade || "?");
  const facts = (r.facts || []).map((f) => f.text).join(" · ");
  return `<tr class="border-t border-edge/60">
      <td class="py-1.5 pr-2"><span class="inline-block w-6 text-center rounded border text-[11px] font-bold ${gradeCls(g, known)}">${escAttr(g)}</span></td>
      <td class="py-1.5 pr-2 font-mono text-[11px] cursor-pointer hover:text-accent" data-wallet-addr="${escAttr(r.query)}">${escAttr(r.query.slice(0, 10))}…${escAttr(r.query.slice(-6))}</td>
      <td class="py-1.5 pr-2 text-[11px] text-slate-400">${escAttr(r.kind)}</td>
      <td class="py-1.5 text-[11px] text-slate-400 truncate max-w-[280px]" title="${escAttr(facts)}">${escAttr(facts || r.verdict || "")}</td>
    </tr>`;
}

function renderBulk(d: BulkResp): string {
  const grades = GRADE_ORDER.filter((g) => d.summary.byGrade[g])
    .map((g) => `<span class="px-2 py-0.5 rounded border text-[11px] ${gradeCls(g, true)}">${g}: ${d.summary.byGrade[g]}</span>`).join(" ");
  const rows = d.results.slice()
    .sort((a, b) => GRADE_ORDER.indexOf(a.coverage?.known === false ? "?" : (a.grade || "?"))
                  - GRADE_ORDER.indexOf(b.coverage?.known === false ? "?" : (b.grade || "?")))
    .map(bulkRow).join("");

  const warn: string[] = [];
  if (d.truncated) warn.push(t("bulk_truncated", { n: String(d.truncated), limit: String(d.limit) }));
  if (d.invalid.length) warn.push(t("bulk_invalid", { n: String(d.invalid.length) }));

  return `
    <div class="rounded-xl border border-edge bg-panel2 p-3 mb-3">
      <div class="flex flex-wrap items-center gap-2 text-[12px]">
        <span class="text-slate-400">${t("bulk_checked")}: <span class="text-slate-100 font-medium">${d.checked}</span></span>
        ${grades}
        <span class="px-2 py-0.5 rounded border border-amber-500/40 bg-amber-500/10 text-amber-400 text-[11px]">${t("bulk_unknown")}: ${d.summary.unknown}</span>
        ${d.summary.withFlags ? `<span class="px-2 py-0.5 rounded border border-bad/40 bg-bad/10 text-bad text-[11px]">${t("bulk_flagged")}: ${d.summary.withFlags}</span>` : ""}
      </div>
      <div class="text-[10px] text-slate-500 mt-1.5">${t("bulk_unknown_note")}</div>
      ${warn.length ? `<div class="text-[11px] text-amber-400/90 mt-1.5">${warn.join(" · ")}</div>` : ""}
    </div>
    <div class="overflow-x-auto"><table class="w-full text-[12px]">
      <thead><tr class="text-[10px] uppercase tracking-wide text-slate-500 text-left">
        <th class="pb-1 font-normal">${t("bulk_col_grade")}</th>
        <th class="pb-1 font-normal">${t("bulk_col_addr")}</th>
        <th class="pb-1 font-normal">${t("bulk_col_kind")}</th>
        <th class="pb-1 font-normal">${t("bulk_col_facts")}</th>
      </tr></thead><tbody>${rows}</tbody>
    </table></div>
    <button id="bulk-csv" class="mt-3 text-[11px] text-accent hover:underline">⬇ ${t("bulk_csv")}</button>`;
}

function toCsv(d: BulkResp): string {
  const esc = (v: string): string => `"${v.replace(/"/g, '""')}"`;
  const head = ["address", "kind", "grade", "known", "verdict", "facts", "flags"].join(",");
  const lines = d.results.map((r) => [
    r.query, r.kind, r.coverage?.known === false ? "?" : (r.grade || ""),
    String(r.coverage?.known !== false), r.verdict || "",
    (r.facts || []).map((f) => f.text).join(" | "), (r.flags || []).join(" | "),
  ].map((x) => esc(String(x))).join(","));
  return [head, ...lines].join("\n");
}

export async function openBulkCheck(): Promise<void> {
  let modal = document.getElementById("bulk-modal");
  if (!modal) {
    modal = document.createElement("div");
    modal.id = "bulk-modal";
    modal.className = "fixed inset-0 z-50 flex items-center justify-center p-4";
    modal.innerHTML = `<div id="bulk-backdrop" class="absolute inset-0 bg-black/70"></div>
      <div class="relative bg-panel border border-edge rounded-2xl w-full max-w-3xl max-h-[88vh] overflow-y-auto p-5 shadow-2xl">
        <div class="flex items-center justify-between mb-3">
          <h2 class="text-base font-semibold">📋 <span id="bulk-title"></span></h2>
          <button id="bulk-close" class="text-slate-400 hover:text-white text-xl leading-none">✕</button>
        </div>
        <div id="bulk-input-wrap">
          <textarea id="bulk-input" rows="5" class="w-full bg-panel2 border border-edge rounded-lg px-3 py-2 text-[12px] font-mono" placeholder="0x…&#10;0x…"></textarea>
          <div class="flex items-center gap-2 mt-2">
            <button id="bulk-go" class="bg-accent/15 hover:bg-accent/25 text-accent border border-accent/30 text-sm px-4 py-1.5 rounded-xl transition"></button>
            <span id="bulk-hint" class="text-[11px] text-slate-500"></span>
          </div>
        </div>
        <div id="bulk-body" class="mt-3"></div>
        <div id="bulk-move"></div>
      </div>`;
    document.body.appendChild(modal);
    modal.querySelector("#bulk-backdrop")!.addEventListener("click", closeBulkCheck);
    modal.querySelector("#bulk-close")!.addEventListener("click", closeBulkCheck);
    modal.querySelector("#bulk-go")!.addEventListener("click", () => void runBulk());
    // Перевірка переказу живе тут же: обидві дії — про «чи варто цьому вірити».
    const mv = modal.querySelector("#bulk-move") as HTMLElement;
    mv.innerHTML = moveCheckSection();
    wireMoveCheck(mv);
  }
  modal.classList.remove("hidden");
  (modal.querySelector("#bulk-title") as HTMLElement).textContent = t("bulk_title");
  (modal.querySelector("#bulk-go") as HTMLElement).textContent = t("bulk_run");
  (modal.querySelector("#bulk-hint") as HTMLElement).textContent = t("bulk_hint");
}

async function runBulk(): Promise<void> {
  const body = document.getElementById("bulk-body") as HTMLElement;
  const raw = (document.getElementById("bulk-input") as HTMLTextAreaElement).value;
  // Будь-який роздільник: кома, пробіл, новий рядок, крапка з комою — список
  // зазвичай приходить із таблиці або з чужого звіту, а не набирається руками.
  const addresses = raw.split(/[\s,;]+/).map((x) => x.trim()).filter(Boolean);
  if (!addresses.length) return;
  body.innerHTML = `<div class="text-slate-400 text-sm py-6 text-center">${t("bulk_running", { n: String(addresses.length) })}</div>`;
  try {
    const res = await fetch("/api/check/bulk", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ addresses }),
    });
    if (!res.ok) throw new Error(String(res.status));
    const d = (await res.json()) as BulkResp;
    body.innerHTML = renderBulk(d);
    body.querySelector("#bulk-csv")?.addEventListener("click", () => {
      const blob = new Blob([toCsv(d)], { type: "text/csv;charset=utf-8" });
      const a = document.createElement("a");
      a.href = URL.createObjectURL(blob);
      a.download = `check-${new Date().toISOString().slice(0, 10)}.csv`;
      a.click();
      URL.revokeObjectURL(a.href);
    });
  } catch {
    body.innerHTML = `<div class="text-bad text-sm py-6 text-center">${t("bulk_err")}</div>`;
  }
}

export function closeBulkCheck(): void {
  document.getElementById("bulk-modal")?.classList.add("hidden");
}

/* ---------- Перевірка переказу: рух ринку чи перекладання всередині біржі ----------

   Найпоширеніша хиба китових стрічок: біржа пересуває власні монети між власними
   гаманцями, а це подають як подію. Тут пара адрес перевіряється за секунду, і видно
   не лише вердикт, а й мітки, на яких він тримається. */

interface MoveResp {
  from: { address: string; label: string | null; exchange: string | null };
  to: { address: string; label: string | null; exchange: string | null };
  verdict: string; isMarketEvent: boolean; explain: string; caveat: string;
  error?: string;
}

function moveHtml(d: MoveResp): string {
  if (d.error) return `<div class="text-[11px] text-bad">${escAttr(d.error)}</div>`;
  const side = (s: MoveResp["from"], title: string): string =>
    `<div class="flex-1 min-w-0">
      <div class="text-[10px] uppercase tracking-wide text-slate-500">${title}</div>
      <div class="font-mono text-[11px] truncate">${escAttr(s.address.slice(0, 10))}…${escAttr(s.address.slice(-6))}</div>
      <div class="text-[11px] text-slate-400 truncate">${s.label ? escAttr(s.label) : `<span class="text-amber-400/80">${t("mv_nolabel")}</span>`}</div>
      ${s.exchange ? `<span class="text-[10px] px-1.5 py-0.5 rounded bg-amber-500/10 text-amber-400 border border-amber-500/30">${escAttr(s.exchange)}</span>` : ""}
    </div>`;
  const cls = d.isMarketEvent
    ? "border-good/40 bg-good/10 text-good"
    : "border-amber-500/40 bg-amber-500/10 text-amber-400";
  return `
    <div class="rounded-xl border ${cls} p-3 mb-2">
      <div class="text-[12px] font-medium">${t("mv_v_" + d.verdict)}</div>
      <div class="text-[11px] mt-0.5 opacity-90">${escAttr(d.explain)}</div>
    </div>
    <div class="flex gap-3 mb-2">${side(d.from, t("mv_from"))}<span class="self-center text-slate-500">→</span>${side(d.to, t("mv_to"))}</div>
    <div class="text-[10px] text-slate-500">${escAttr(d.caveat)}</div>`;
}

export function moveCheckSection(): string {
  return `
    <div class="rounded-xl border border-edge bg-panel2 p-3 mt-4">
      <div class="text-xs font-medium mb-1.5">${t("mv_title")}</div>
      <div class="text-[10px] text-slate-500 mb-2">${t("mv_hint")}</div>
      <div class="flex gap-2">
        <input id="mv-from" placeholder="${t("mv_from")} 0x…" class="flex-1 min-w-0 bg-panel border border-edge rounded-lg px-2 py-1 text-[11px] font-mono" />
        <input id="mv-to" placeholder="${t("mv_to")} 0x…" class="flex-1 min-w-0 bg-panel border border-edge rounded-lg px-2 py-1 text-[11px] font-mono" />
        <button id="mv-go" class="shrink-0 bg-accent/15 hover:bg-accent/25 text-accent border border-accent/30 text-[11px] px-3 rounded-lg">${t("mv_run")}</button>
      </div>
      <div id="mv-body" class="mt-2"></div>
    </div>`;
}

export function wireMoveCheck(root: HTMLElement): void {
  const go = root.querySelector("#mv-go");
  if (!go) return;
  go.addEventListener("click", () => void (async () => {
    const out = root.querySelector("#mv-body") as HTMLElement;
    const from = (root.querySelector("#mv-from") as HTMLInputElement).value.trim();
    const to = (root.querySelector("#mv-to") as HTMLInputElement).value.trim();
    if (!from || !to) { out.innerHTML = `<div class="text-[11px] text-slate-500">${t("mv_need_both")}</div>`; return }
    out.innerHTML = `<div class="text-[11px] text-slate-500">${t("mv_checking")}</div>`;
    try { out.innerHTML = moveHtml(await getJSON<MoveResp>("/api/move-check", { from, to })); }
    catch { out.innerHTML = `<div class="text-[11px] text-bad">${t("mv_err")}</div>`; }
  })());
}
