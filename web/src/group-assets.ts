/* Активи груп капіталу — частка емісії, яку тримає група в кожному активі.
 *
 * Навіщо окремий модуль: у capital-groups.json поле assets порожнє у всіх груп,
 * тож зв'язку «група → її активи» в інтерфейсі не було взагалі. Дані рахує
 * deploy/build-group-assets.py і віддає /api/group-assets.
 *
 * Клікабельність наскрізна: група розгортається → актив відкриває холдерів →
 * адреса відкриває картку гаманця. Жоден рівень не є глухим кутом.
 */
import { getJSON } from "./api";
import { openAssetHolders } from "./holders";

interface GAsset {
  symbol: string;
  contract: string | null;
  name: string | null;
  rank: number | null;
  top10pct: number | null;
  kind: "altcoin" | "stable" | "wrapped" | "major";
  emissionShare: number;
  shareRaw: number;
  capped: boolean;
  belowThreshold?: boolean;
  holders: number;
}

interface GGroup {
  id: string;
  name: string | null;
  size: number | null;
  totalUsd: number | null;
  assetsCount: number;
  altcoinsCount: number;
  topShare: number;
  assets: GAsset[];
}

interface GAResponse {
  groupsTotal?: number;
  groupsWithAssets?: number;
  assetsTotal?: number;
  minShare?: number;
  caveat?: string;
  groups: GGroup[];
}

let data: GAResponse | null = null;
let onlyAlt = false;
let query = "";
const expanded = new Set<string>();

const esc = (s: string) =>
  s.replace(/[&<>"']/g, (c) =>
    ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c] as string));

const usd = (v: number | null) =>
  v == null ? "—" : v >= 1e9 ? `$${(v / 1e9).toFixed(2)} млрд`
    : v >= 1e6 ? `$${(v / 1e6).toFixed(1)} млн`
    : v >= 1e3 ? `$${(v / 1e3).toFixed(0)} тис.` : `$${v.toFixed(0)}`;

/** Колір за часткою: 40–60% — зона контролю, >70% — пастка ліквідності. */
function shareTone(p: number): string {
  if (p >= 70) return "text-rose-400";
  if (p >= 40) return "text-amber-400";
  if (p >= 10) return "text-sky-400";
  return "text-slate-400";
}

function kindBadge(k: GAsset["kind"]): string {
  const map: Record<GAsset["kind"], [string, string]> = {
    altcoin: ["альт", "bg-sky-500/15 text-sky-300"],
    stable: ["стейбл", "bg-slate-500/15 text-slate-300"],
    wrapped: ["обгортка", "bg-slate-500/15 text-slate-300"],
    major: ["мажор", "bg-violet-500/15 text-violet-300"],
  };
  const [label, cls] = map[k];
  return `<span class="px-1.5 py-0.5 rounded text-[10px] ${cls}">${label}</span>`;
}

function assetRow(g: GGroup, a: GAsset): string {
  const pct = a.emissionShare.toFixed(2);
  const capped = a.capped
    ? ` <span title="сума часток перевищила 100% — адреса трапляється в кількох джерелах" class="text-[10px] text-amber-500">≈</span>`
    : "";
  return `
    <button type="button"
        class="w-full text-left px-3 py-1.5 hover:bg-white/5 rounded flex items-center gap-2 group"
        data-ga-asset="${esc(a.symbol)}" data-ga-contract="${esc(a.contract || "")}">
      <span class="font-mono text-xs w-28 shrink-0 truncate">${esc(a.symbol)}</span>
      ${kindBadge(a.kind)}
      <span class="ml-auto font-mono text-xs ${shareTone(a.emissionShare)}">${pct}%${capped}</span>
      <span class="text-[10px] text-slate-500 w-16 text-right">${a.holders} адр.</span>
      <span class="text-[10px] text-slate-600 opacity-0 group-hover:opacity-100">холдери →</span>
    </button>`;
}

function groupCard(g: GGroup): string {
  const isOpen = expanded.has(g.id);
  let list = g.assets;
  if (onlyAlt) list = list.filter((a) => a.kind === "altcoin");
  if (query) {
    const q = query.toLowerCase();
    list = list.filter((a) => a.symbol.toLowerCase().includes(q));
  }

  const rows = isOpen
    ? (list.length
        ? list.map((a) => assetRow(g, a)).join("")
        : `<p class="px-3 py-2 text-xs text-slate-500">За фільтром нічого не знайдено.</p>`)
    : "";

  return `
    <div class="border border-edge rounded-xl overflow-hidden bg-panel/40">
      <button type="button" class="w-full text-left px-3 py-2 hover:bg-white/5 flex items-center gap-3"
              data-ga-group="${esc(g.id)}" aria-expanded="${isOpen}">
        <span class="font-mono text-sm font-semibold">${esc(g.id)}</span>
        <span class="text-[11px] text-slate-500">${g.size ?? "—"} адрес · ${usd(g.totalUsd)}</span>
        <span class="ml-auto text-[11px] text-slate-400">
          ${g.assetsCount} активів · <span class="text-sky-400">${g.altcoinsCount} альт</span>
        </span>
        ${g.assets[0]?.belowThreshold
          ? `<span class="px-1.5 py-0.5 rounded text-[10px] bg-slate-500/15 text-slate-400"
                   title="немає активів понад 20% емісії — показано топ позицій">нижче 20%</span>`
          : ""}
        <span class="font-mono text-xs ${shareTone(g.topShare)}">${g.topShare.toFixed(1)}%</span>
        <span class="text-slate-500 text-xs">${isOpen ? "▾" : "▸"}</span>
      </button>
      ${isOpen ? `<div class="border-t border-edge py-1">${rows}</div>` : ""}
    </div>`;
}

function render(): void {
  const el = document.getElementById("group-assets");
  if (!el || !data) return;

  const total = data.groups.reduce((n, g) => n + g.assetsCount, 0);
  const alts = data.groups.reduce((n, g) => n + g.altcoinsCount, 0);

  el.innerHTML = `
    <div class="flex flex-wrap items-center gap-2 mb-3">
      <input id="ga-search" type="search" placeholder="пошук активу…" value="${esc(query)}"
             class="bg-panel border border-edge rounded-lg px-2 py-1 text-xs w-40" />
      <label class="flex items-center gap-1.5 text-xs text-slate-400 cursor-pointer">
        <input id="ga-only-alt" type="checkbox" ${onlyAlt ? "checked" : ""} class="accent-sky-500" />
        тільки альткоїни
      </label>
      <span class="ml-auto text-[11px] text-slate-500">
        ${data.groupsWithAssets ?? 0} з ${data.groupsTotal ?? 0} груп · ${total} активів · ${alts} альт · актив може бути в кількох групах
      </span>
    </div>
    <div class="space-y-2">${data.groups.map(groupCard).join("")}</div>
    ${data.caveat ? `<p class="mt-3 text-[10px] text-slate-600 leading-relaxed">${esc(data.caveat)}</p>` : ""}`;
}

/** Делеговані кліки: розгортання групи, відкриття холдерів активу, фільтри. */
function wire(): void {
  const el = document.getElementById("group-assets");
  if (!el || el.dataset.wired) return;
  el.dataset.wired = "1";

  el.addEventListener("click", (e: Event) => {
    const t = e.target as HTMLElement;

    const grp = t.closest<HTMLElement>("[data-ga-group]");
    if (grp) {
      const id = grp.dataset.gaGroup!;
      expanded.has(id) ? expanded.delete(id) : expanded.add(id);
      render();
      return;
    }

    const asset = t.closest<HTMLElement>("[data-ga-asset]");
    if (asset) {
      const sym = asset.dataset.gaAsset!;
      // холдери відкриваються наявним модулем — далі адреса веде в картку гаманця
      openAssetHolders(sym);
    }
  });

  el.addEventListener("input", (e: Event) => {
    const t = e.target as HTMLInputElement;
    if (t.id === "ga-search") {
      query = t.value.trim();
      render();
      const again = document.getElementById("ga-search") as HTMLInputElement | null;
      if (again) { again.focus(); again.setSelectionRange(again.value.length, again.value.length); }
    }
  });

  el.addEventListener("change", (e: Event) => {
    const t = e.target as HTMLInputElement;
    if (t.id === "ga-only-alt") { onlyAlt = t.checked; render(); }
  });
}

export async function loadGroupAssets(): Promise<void> {
  const el = document.getElementById("group-assets");
  if (!el) return;
  el.innerHTML = `<p class="text-xs text-slate-500">Завантаження активів груп…</p>`;
  try {
    data = await getJSON<GAResponse>("/api/group-assets");
    if (!data.groups?.length) {
      el.innerHTML = `<p class="text-xs text-slate-500">Даних про активи груп ще немає.</p>`;
      return;
    }
    // найбільша група розгорнута одразу — щоб екран не був порожнім
    if (!expanded.size) expanded.add(data.groups[0].id);
    wire();
    render();
  } catch {
    el.innerHTML = `<p class="text-xs text-rose-400">Не вдалося завантажити активи груп.</p>`;
  }
}
