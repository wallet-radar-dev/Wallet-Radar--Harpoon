/* ---------- Рендер: чисті продюсери HTML (жодних відкривачів фіч) ----------
   Функції лише будують розмітку з даними-атрибутами (data-*); диспетчеризацію
   кліків робить композиційний корінь (init у main.ts) через делегування. */

import type { BlockchainEvent, EventKind, Wallet } from "./types";
import { t, lang } from "./i18n";
import { NETWORKS, networkById } from "./networks";
import { state } from "./state";
import { $, fmtUsd, fmtNum, fmtTime, shortAddr } from "./util";
import { ENTITY_TYPE_CLS } from "./attribution";
import { balanceStatusOf, observeWalletList } from "./wallet-balances";

export const KIND_META: Record<EventKind, { icon: string; cls: string }> = {
  in: { icon: "⬇", cls: "text-good" },
  out: { icon: "⬆", cls: "text-bad" },
  swap: { icon: "🔄", cls: "text-sky-400" },
  approval: { icon: "✍", cls: "text-yellow-400" },
};

export const kindLabel = (k: EventKind): string => t(`kind_${k}`);

// Локалізований текст події «група капіталу» з category + полів (бо ev.title/ev.body — серверні, україномовні).
export function groupEventText(ev: BlockchainEvent): { title: string; body: string } {
  const sym = (ev.assets || [])[0] || "";
  const n = (ev.members || []).length;
  const val = ev.valueUsd ? fmtUsd(ev.valueUsd) : "";
  const assets = (ev.assets || []).slice(0, 6).join(", ");
  switch (ev.category) {
    case "conductor": return { title: t("ev_conductor"), body: [assets, val].filter(Boolean).join(" · ") };
    case "billion-group": return { title: t("ev_bgroup"), body: [`${n} ${t("ev_members")}`, val].filter(Boolean).join(" · ") };
    case "billion-whale": return { title: t("ev_bwhale"), body: [val, assets].filter(Boolean).join(" · ") };
    case "conductor-funding": return { title: t("ev_funding"), body: val };
    case "smart-money": return { title: t("ev_smart"), body: assets };
    case "active-group": return { title: t("ev_active"), body: assets };
    case "whale-trace": return { title: `${t("ev_trace")} ${sym}`, body: val };
    case "coordinated-move": return { title: `${(ev.icon === "🔴" ? t("ev_move_dist") : t("ev_move_acc"))} ${sym}`, body: `${n} ${t("ev_members")}` };
    case "liquidation": {
      const mt = ev.meta || {};
      const parts = [];
      if (mt.long != null) parts.push(`${t("ev_liq_long")} ${fmtUsd(mt.long)}`);
      if (mt.short != null) parts.push(`${t("ev_liq_short")} ${fmtUsd(mt.short)}`);
      if (assets) parts.push(assets);
      return { title: `${t("ev_liq")} · ${val}/5${lang === "en" ? "m" : lang === "de" ? "Min" : lang === "fr" ? "min" : "хв"}`, body: parts.join(" · ") };
    }
    default: return { title: ev.title || "", body: ev.body || "" };
  }
}

export function renderNetworkCheckboxes(): void {
  const box = $<HTMLDivElement>("#net-checkboxes");
  box.innerHTML = NETWORKS.map(
    (n) => `
      <label class="flex items-center gap-2 bg-panel2 border border-edge rounded-lg px-2.5 py-1.5 cursor-pointer hover:border-accent transition text-sm select-none">
        <input type="checkbox" value="${n.id}" class="net-check accent-blue-500 w-4 h-4" />
        <span class="${n.color} truncate">${n.name}</span>
      </label>`
  ).join("");
}

export const selectedNetworks = (): string[] =>
  [...document.querySelectorAll<HTMLInputElement>(".net-check:checked")].map((c) => c.value);

export function renderStats(): void {
  // «Сумарна вартість» рахує ЛИШЕ реальні ончейн-баланси. Раніше вона підсумовувала w.valueUsd, а це
  // для незавантаженого гаманця — синтетична оцінка з holdings (у трьох початкових гаманців там
  // лежать демо-числа з часів, коли бекенда ще не було). Виходило $7.87M «сумарної вартості» при
  // жодному відомому балансі — вигадка з виглядом підсумку. Невідомий баланс тепер не додає нічого,
  // а підпис під числом каже, за скільки гаманців воно взагалі пораховане.
  const known = state.wallets.filter((w) => typeof w.realUsd === "number");
  const totalValue = known.reduce((s, w) => s + (w.realUsd as number), 0);
  const activeNets = new Set(state.wallets.flatMap((w) => w.networks)).size;
  $("#stat-wallets").textContent = String(state.wallets.length);
  $("#stat-value").textContent = fmtUsd(totalValue);
  $("#stat-events").textContent = String(state.eventsToday);
  $("#stat-networks").textContent = String(activeNets);

  const note = document.querySelector<HTMLElement>("#stat-value-note");
  if (note) {
    note.textContent =
      known.length < state.wallets.length
        ? t("stat_value_partial", { n: String(known.length), m: String(state.wallets.length) })
        : "";
  }
}

/**
 * Рядок балансу гаманця. Раніше тут було двійкове «є realUsd / ⏳ вантажиться», і друге показувалось
 * навіть тоді, коли ніхто нічого не вантажив (запит упав, помилку з'їв catch) — вічний обманний
 * спінер. Тепер стан гаманця називається своїм іменем.
 */
function walletBalanceLine(w: Wallet): string {
  if (typeof w.realUsd === "number") {
    return `${fmtUsd(w.realUsd)} · ${w.realTokens ?? w.holdings.length} ${t("pf_assets").toLowerCase()}`;
  }
  switch (balanceStatusOf(w.id)) {
    case "loading":
      return `<span class="text-slate-600">⏳ ${t("wl_loading")}</span>`;
    case "queued":
      return `<span class="text-slate-600">· ${t("wl_queued")}</span>`;
    case "error":
      // Клікабельно: click-хендлер списку впізнає [data-retry] і повторює запит.
      return `<button type="button" data-retry="${w.id}" class="text-yellow-500 hover:underline">⚠ ${t("wl_failed")}</button>`;
    default:
      return `<span class="text-slate-600">${t("wl_idle")}</span>`;
  }
}

export function renderWallets(): void {
  const list = $<HTMLUListElement>("#wallet-list");
  $("#wallet-count").textContent = `${state.wallets.length} ${t("wallets_count_suffix")}`.trim();
  if (state.wallets.length === 0) {
    list.innerHTML = `<li class="text-sm text-slate-500 py-4 text-center">${t("wallets_empty")}</li>`;
    return;
  }
  list.innerHTML = state.wallets
    .map((w) => {
      const chips = w.networks
        .map((id) => {
          const net = networkById(id);
          return `<span class="text-xs font-medium ${net.color}">${net.name}</span>`;
        })
        .join('<span class="text-slate-600 text-xs">·</span>');
      return `
      <li data-wallet="${w.id}" title="${t("pf_click_hint")}"
          class="bg-panel2 border border-edge rounded-xl p-3 flex items-center justify-between gap-2 cursor-pointer hover:border-accent transition">
        <div class="min-w-0">
          <div class="flex items-center gap-1.5 flex-wrap">
            ${chips}
            ${w.label ? `<span class="text-xs text-slate-400 truncate">· ${w.label}</span>` : ""}
          </div>
          <div class="font-mono text-sm text-slate-300 truncate">${shortAddr(w.address)}</div>
          <div class="text-xs text-slate-500">${walletBalanceLine(w)}</div>
        </div>
        <button data-del="${w.id}" class="shrink-0 text-slate-500 hover:text-bad text-lg leading-none px-1" title="Remove">×</button>
      </li>`;
    })
    .join("");

  // Список щойно перебудовано з нуля (innerHTML) — старі <li> зникли разом зі спостереженням.
  // Без цього рядка лінива підкачка балансів мовчки перестала б працювати після першого перерендеру.
  observeWalletList();
}

/** Будує DOM-елемент одного рядка стрічки (поточною мовою). */
export function eventLi(ev: BlockchainEvent): HTMLLIElement {
  // подія «група капіталу» (як у Telegram-боті) — клік відкриває повну картину
  if (ev.kind as string === "group") {
    const gt = groupEventText(ev);
    const li = document.createElement("li");
    li.className = "px-4 py-3 flex items-center gap-3 animate-slidein cursor-pointer hover:bg-fuchsia-500/5 transition group border-l-2 border-fuchsia-500/50";
    li.dataset.ev = ev.id;
    li.title = t("click_for_details");
    li.innerHTML = `
      <div class="w-9 h-9 rounded-lg bg-fuchsia-500/15 grid place-items-center text-lg shrink-0">${ev.icon || "🔔"}</div>
      <div class="min-w-0 flex-1">
        <div class="text-sm font-medium text-fuchsia-200 truncate">${gt.title}</div>
        <div class="text-xs text-slate-400 truncate">${gt.body}</div>
      </div>
      ${ev.valueUsd ? `<div class="text-xs text-good shrink-0">${fmtUsd(ev.valueUsd)}</div>` : ""}
      <div class="text-xs text-slate-500 shrink-0">${fmtTime(ev.timestamp)}</div>
      <div class="text-slate-600 group-hover:text-accent shrink-0 text-lg leading-none">›</div>`;
    return li;
  }
  const wallet = state.wallets.find((w) => w.id === ev.walletId);
  const net = networkById(ev.network);
  const meta = KIND_META[ev.kind];
  // Бейдж контрагента — прикраса, а не привід валити рендер: одна крива подія не має права
  // забрати з собою всю стрічку (а через catch у startData — і живий WebSocket разом із нею).
  const ent = ev.counterpartyEntity;
  const entBadge =
    ent && ent.type !== "unknown" && ENTITY_TYPE_CLS[ent.type]
      ? `<span class="text-[11px] px-1.5 py-0.5 rounded ${ENTITY_TYPE_CLS[ent.type]} shrink-0" title="Arkham Intelligence">${ent.name}</span>`
      : "";

  const li = document.createElement("li");
  li.className = "px-4 py-3 flex items-center gap-3 animate-slidein cursor-pointer hover:bg-panel2/60 transition group";
  li.dataset.ev = ev.id;
  li.title = t("click_for_details");
  li.innerHTML = `
    <div class="w-9 h-9 rounded-lg bg-panel2 grid place-items-center ${meta.cls} text-lg">${meta.icon}</div>
    <div class="min-w-0 flex-1">
      <div class="flex items-center gap-2 text-sm">
        <span class="font-medium ${meta.cls}">${kindLabel(ev.kind)}</span>
        <span class="${net.color} text-xs">${net.name}</span>
        <span class="text-slate-500 text-xs truncate">${wallet?.label || shortAddr(wallet?.address ?? "")}</span>
        ${entBadge}
      </div>
      <div class="text-xs text-slate-400 font-mono truncate">
        ${fmtNum(ev.amount)} ${ev.asset}
        · ${fmtUsd(ev.valueUsd)} · ${ev.kind === "out" ? "→" : "←"} ${shortAddr(ev.counterparty)}
      </div>
    </div>
    <div class="text-xs text-slate-500 shrink-0">${fmtTime(ev.timestamp)}</div>
    <div class="text-slate-600 group-hover:text-accent shrink-0 text-lg leading-none">›</div>`;
  return li;
}

export function prependEvent(ev: BlockchainEvent): void {
  const feed = $<HTMLUListElement>("#event-feed");
  $("#feed-empty")?.remove();
  feed.prepend(eventLi(ev));
  while (feed.children.length > 60) feed.lastElementChild?.remove();
}

/** Повністю перебудовує стрічку (напр. після зміни мови). */
export function rebuildFeed(): void {
  const feed = $<HTMLUListElement>("#event-feed");
  if (state.events.length === 0) {
    feed.innerHTML = `<li class="px-4 py-10 text-center text-slate-500 text-sm" id="feed-empty" data-i18n="feed_empty">${t("feed_empty")}</li>`;
    return;
  }
  feed.innerHTML = "";
  state.events.slice(0, 60).forEach((ev) => feed.append(eventLi(ev)));
}
