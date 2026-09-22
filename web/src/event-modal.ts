/* ---------- Модальне вікно: повні деталі події ---------- */

import type { BlockchainEvent } from "./types";
import { t } from "./i18n";
import { $, fmtUsd, fmtNum, fmtDateTime, localeOf } from "./util";
import { state } from "./state";
import { escAttr, link, row } from "./ui";
import { networkById, explorerAddr, explorerTx } from "./networks";
import { arkhamLink, entityBadge } from "./attribution";
import { KIND_META, kindLabel, groupEventText } from "./render";
import { nav } from "./nav";


// Повна картина події «група капіталу»: адреса → портфель (з вердиктом/трасуванням); група → статистика всіх гаманців.
export function openGroupAlert(ev: BlockchainEvent): void {
  if (ev.address && /^0x[0-9a-fA-F]{40}$/.test(ev.address)) { nav.openWallet(ev.address, ev.title || ""); return; }
  const members = (ev.members || []).filter((m) => /^0x[0-9a-fA-F]{40}$/.test(m));
  if (!members.length) {
    // Подія без адреси/членів (напр. ліквідаційний флеш) — просте інфо-вікно з деталями.
    if (ev.category === "liquidation") {
      const gt = groupEventText(ev);
      const top = (ev.meta?.top || []).slice(0, 6);
      const b0 = $("#portfolio-body");
      b0.innerHTML = `<div>
        <div class="text-base font-semibold text-fuchsia-300">${gt.title}</div>
        <div class="text-sm text-slate-300 mt-2">${gt.body}</div>
        ${top.length ? `<div class="text-[11px] text-slate-400 mt-3 mb-1">📊 ${t("liq_top")}:</div>
        <div class="space-y-0.5">${top.map((x) => `<div class="flex justify-between text-[12px]"><span class="font-mono">${x.symbol}</span><span class="text-slate-400">${fmtUsd(x.usd)}</span></div>`).join("")}</div>` : ""}</div>`;
      const m0 = $("#portfolio-modal"); m0.classList.remove("hidden"); m0.classList.add("flex");
      return;
    }
    openEventModal(ev); return;
  }
  const body = $("#portfolio-body");
  body.innerHTML = `<div>
    <div class="text-base font-semibold text-fuchsia-300">${ev.icon || ""} ${groupEventText(ev).title}</div>
    <div class="text-xs text-slate-400 mt-1 mb-2">${groupEventText(ev).body}</div>
    ${(ev.assets || []).length ? `<div class="text-[11px] text-slate-500 mb-2">📦 ${(ev.assets || []).join(", ")}</div>` : ""}
    <div data-group-stats class="mt-1 text-[11px] text-slate-500">${t("cg_loading")}</div></div>`;
  const modal = $("#portfolio-modal"); modal.classList.remove("hidden"); modal.classList.add("flex");
  const cont = body.querySelector<HTMLElement>("[data-group-stats]");
  if (cont) void nav.groupStats(members.join(","), cont);
}

export function openEventModal(ev: BlockchainEvent): void {
  const wallet = state.wallets.find((w) => w.id === ev.walletId);
  const net = networkById(ev.network);
  const meta = KIND_META[ev.kind];
  const dirText = ev.kind === "out" ? t("dir_out") : ev.kind === "in" ? t("dir_in") : t("dir_internal");
  const walletLabel = wallet?.label ? `${wallet.label} · ` : "";

  $("#modal-body").innerHTML = `
    <div class="flex items-center gap-3">
      <div class="w-12 h-12 rounded-xl bg-panel2 grid place-items-center ${meta.cls} text-2xl">${meta.icon}</div>
      <div>
        <div class="flex items-center gap-2">
          <span class="text-lg font-semibold ${meta.cls}">${kindLabel(ev.kind)}</span>
          <span class="text-xs px-2 py-0.5 rounded-full bg-good/15 text-good">${ev.status === "confirmed" ? t("status_confirmed") : t("status_pending")}</span>
        </div>
        <div class="text-sm ${net.color}">${net.name}</div>
      </div>
    </div>

    <div class="bg-panel2 border border-edge rounded-xl p-4 flex items-baseline justify-between">
      <div class="text-2xl font-semibold ${meta.cls}">
        ${ev.kind === "out" ? "−" : ev.kind === "in" ? "+" : ""}${fmtNum(ev.amount, 6)} ${ev.asset}
      </div>
      <div class="text-slate-300">${fmtUsd(ev.valueUsd)}</div>
    </div>

    <dl>
      ${row(t("m_type"), kindLabel(ev.kind))}
      ${row(t("m_direction"), dirText)}
      ${row(t("m_network"), `<span class="${net.color}">${net.name}</span> <span class="text-slate-500">(${net.symbol})</span>`)}
      ${row(t("m_asset"), ev.asset)}
      ${row(t("m_amount"), `${fmtNum(ev.amount, 8)} ${ev.asset}`)}
      ${row(t("m_value"), fmtUsd(ev.valueUsd))}
      ${row(t("m_fee"), `$${ev.feeUsd.toFixed(2)}`)}
      ${row(t("m_wallet"), `
        <div>${walletLabel}<span class="font-mono">${wallet ? link(explorerAddr(net, wallet.address), wallet.address) : "—"}</span></div>
        ${wallet ? `<div class="mt-1 flex flex-wrap items-center gap-2"><button data-wallet-addr="${escAttr(wallet.address)}" class="text-xs text-accent hover:underline">📊 ${t("pf_open")}</button><a href="${arkhamLink(wallet.address)}" target="_blank" rel="noopener" class="text-xs text-purple-400 hover:underline">🔍 Arkham →</a></div>` : ""}`)}
      ${row(t("m_counterparty"), `
        <div class="font-mono">${link(explorerAddr(net, ev.counterparty), ev.counterparty)}</div>
        <div class="mt-1.5 flex flex-wrap items-center gap-2">
          ${entityBadge(ev.counterpartyEntity)}
          <button data-wallet-addr="${escAttr(ev.counterparty)}" class="text-xs text-accent hover:underline">📊 ${t("pf_open")}</button>
          ${ev.counterpartyEntity.type !== "unknown" ? `<button data-entity-name="${escAttr(ev.counterpartyEntity.name)}" data-entity-type="${ev.counterpartyEntity.type}" class="text-xs text-purple-400 hover:underline">${t("ent_profile_btn")}</button>` : ""}
          <a href="${arkhamLink(ev.counterparty)}" target="_blank" rel="noopener" class="text-xs text-purple-400 hover:underline">🔍 Arkham →</a>
        </div>`)}
      ${row(t("m_txhash"), `<span class="font-mono">${link(explorerTx(net, ev.txHash), ev.txHash)}</span>`)}
      ${row(t("m_block"), `#${ev.blockNumber.toLocaleString(localeOf())}`)}
      ${row(t("m_time"), fmtDateTime(ev.timestamp))}
      ${row(t("m_eventid"), `<span class="font-mono text-slate-400">${ev.id}</span>`)}
    </dl>

    <div class="grid grid-cols-2 gap-2">
      <a href="${explorerTx(net, ev.txHash)}" target="_blank" rel="noopener"
         class="block text-center bg-accent hover:bg-blue-500 text-white text-sm font-medium py-2 rounded-xl transition">
        ${t("btn_explorer")} (${net.name})
      </a>
      <a href="${arkhamLink(ev.counterparty)}" target="_blank" rel="noopener"
         class="block text-center bg-purple-600 hover:bg-purple-500 text-white text-sm font-medium py-2 rounded-xl transition">
        🔍 Arkham Intel
      </a>
    </div>`;

  const modal = $("#event-modal");
  modal.classList.remove("hidden");
  modal.classList.add("flex");
}

export function closeEventModal(): void {
  const modal = $("#event-modal");
  modal.classList.add("hidden");
  modal.classList.remove("flex");
}


