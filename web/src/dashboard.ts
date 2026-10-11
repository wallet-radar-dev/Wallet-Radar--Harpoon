/* ---------- Custom Dashboards (віджети) ---------- */

import type { Wallet, DashWidget } from "./types";
import { t } from "./i18n";
import { $, uid, fmtUsd, fmtPrice, shortAddr } from "./util";
import { state } from "./state";
import { escAttr } from "./ui";
import { priceOf } from "./prices";
import { ASSET_DIR } from "./assets";


function resolveWallet(target: string): Wallet | undefined {
  const tg = target.toLowerCase();
  return state.wallets.find((w) => w.id === target || (w.label || "").toLowerCase().includes(tg) || w.address.toLowerCase().includes(tg));
}

/** Тримачі активу серед ВІДСТЕЖУВАНИХ гаманців.
 *
 *  Раніше сюди домішувались вісім зашитих сутностей із балансами `50M + rnd()*1.5B` —
 *  тобто віджет «топ тримачів» показував вигадані мільярди як факт. Список холдерів
 *  усього ланцюга віджет дати не може (це мережевий запит на кожне перемальовування),
 *  тому він чесно обмежується тим, що є в застосунку, і так і підписаний. */
function ownedBy(sym: string, n: number): { name: string; value: number }[] {
  const price = priceOf(sym) || 0;
  const rows: { name: string; value: number }[] = [];
  state.wallets.forEach((w) => {
    const h = w.holdings.find((x) => x.asset === sym);
    if (h) rows.push({ name: w.label || shortAddr(w.address), value: h.amount * price });
  });
  return rows.sort((a, b) => b.value - a.value).slice(0, n);
}

function widgetCard(w: DashWidget): string {
  let title = "", body = "";
  if (w.type === "total") {
    const total = state.wallets.reduce((s, x) => s + x.valueUsd, 0);
    title = t("dash_widget_total");
    body = `<div class="text-2xl font-semibold">${fmtUsd(total)}</div><div class="text-xs text-slate-400">${state.wallets.length} ${t("wallets").toLowerCase()}</div>`;
  } else if (w.type === "price") {
    const sym = (w.target || "").toUpperCase(); const p = priceOf(sym);
    title = `${ASSET_DIR.find((a) => a.sym === sym)?.name ?? sym} (${sym})`;
    body = `<div class="text-2xl font-semibold">${p ? fmtPrice(p) : "—"}</div><div class="text-xs text-slate-400">${t("dash_widget_price")}</div>`;
  } else if (w.type === "wallet") {
    const wl = resolveWallet(w.target);
    title = wl ? (wl.label || shortAddr(wl.address)) : t("dash_widget_wallet");
    body = wl
      ? `<div data-wallet-addr="${escAttr(wl.address)}" class="cursor-pointer hover:text-accent"><div class="text-xl font-semibold">${fmtUsd(wl.valueUsd)}</div><div class="text-xs text-slate-400">${wl.holdings.length} ${t("pf_assets").toLowerCase()}</div></div>`
      : `<div class="text-xs text-slate-500">—</div>`;
  } else {
    const sym = (w.target || "ETH").toUpperCase();
    title = `${t("holders_ours")}: ${sym}`;
    const rows = ownedBy(sym, 3);
    body = rows.length
      ? rows.map((r, i) => `<div class="flex justify-between gap-2 text-xs py-0.5"><span class="truncate">${i + 1}. ${r.name}</span><span class="text-slate-400 shrink-0">${fmtUsd(r.value)}</span></div>`).join("")
      : `<div class="text-xs text-slate-500">${t("dash_no_holders")}</div>`;
  }
  return `<div class="bg-panel2 border border-edge rounded-xl p-3 relative">
    <button data-del-widget="${w.id}" class="absolute top-1.5 right-2 text-slate-600 hover:text-bad text-base leading-none">×</button>
    <div class="text-[11px] text-slate-400 mb-1 pr-4 truncate">${title}</div>${body}</div>`;
}

function fillDashType(): void {
  const sel = $<HTMLSelectElement>("#dash-type"); const pv = sel.value;
  const opts: [DashWidget["type"], string][] = [["total", "dash_widget_total"], ["price", "dash_widget_price"], ["wallet", "dash_widget_wallet"], ["holders", "holders_title"]];
  sel.innerHTML = opts.map(([v, k]) => `<option value="${v}">${t(k)}</option>`).join("");
  if (pv) sel.value = pv;
}

export function renderDashboard(): void {
  fillDashType();
  $("#dash-grid").innerHTML = state.dashboard.length
    ? state.dashboard.map(widgetCard).join("")
    : `<div class="text-xs text-slate-500 col-span-full">${t("dash_empty")}</div>`;
}

export function addDashWidget(): void {
  const type = $<HTMLSelectElement>("#dash-type").value as DashWidget["type"];
  const target = $<HTMLInputElement>("#dash-target").value.trim();
  state.dashboard.push({ id: uid(), type, target });
  $<HTMLInputElement>("#dash-target").value = "";
  renderDashboard();
}

export function openDashboard(): void {
  renderDashboard();
  const m = $("#dash-modal");
  m.classList.remove("hidden"); m.classList.add("flex");
}

export function closeDashboard(): void {
  const m = $("#dash-modal");
  m.classList.add("hidden"); m.classList.remove("flex");
}

