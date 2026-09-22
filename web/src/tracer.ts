/* ---------- Tracer: трасування потоків коштів (Arkham-style) ---------- */

import type { Wallet, EntityType } from "./types";
import { t } from "./i18n";
import { $, fmtUsd, fmtNum, shortAddr, randomHex, seededRand } from "./util";
import { state } from "./state";
import { escAttr } from "./ui";
import { priceOf } from "./prices";
import { networkById, EVM_CHAINS } from "./networks";
import { arkhamLink, ARKHAM_ENTITIES, ENTITY_TYPE_CLS, ENTITY_HEX, WALLET_HEX } from "./attribution";


type TraceAction = "transfer" | "bridge" | "swap" | "cex_deposit";

interface TraceHop {
  action: TraceAction; toName: string; toKind: "wallet" | "entity"; toEtype: EntityType | null;
  toAddr: string | null; asset: string; amount: number; value: number; fromNet: string; toNet: string;
}

const ACTION_META: Record<TraceAction, { key: string; icon: string; cls: string }> = {
  transfer: { key: "action_transfer", icon: "➡", cls: "text-slate-300" },
  bridge: { key: "action_bridge", icon: "🌉", cls: "text-sky-400" },
  swap: { key: "action_swap", icon: "🔄", cls: "text-purple-400" },
  cex_deposit: { key: "action_cex", icon: "🏦", cls: "text-yellow-400" },
};

let tracerWid: string | null = null;

function buildTrace(w: Wallet): TraceHop[] {
  const rnd = seededRand("trace:" + w.address);
  const hops: TraceHop[] = [];
  let curNet = w.networks[0];
  let asset = w.holdings[0]?.asset ?? networkById(curNet).symbol;
  let amount = (w.holdings[0]?.amount ?? 100) * (0.15 + rnd() * 0.4);
  const cex = ["Binance", "Coinbase", "OKX", "Kraken"];
  const steps = 4 + Math.floor(rnd() * 3);
  for (let i = 0; i < steps; i++) {
    const last = i === steps - 1;
    const roll = rnd();
    let action: TraceAction = last ? "cex_deposit" : roll < 0.22 ? "bridge" : roll < 0.42 ? "swap" : roll < 0.6 ? "cex_deposit" : "transfer";
    let toNet = curNet;
    if (action === "bridge") {
      const opts = EVM_CHAINS.filter((c) => c !== curNet);
      toNet = opts[Math.floor(rnd() * opts.length)];
    }
    if (action === "swap") asset = ["USDT", "USDC", "ETH", "WBTC"][Math.floor(rnd() * 4)];

    let toName: string, toKind: "wallet" | "entity", toEtype: EntityType | null;
    const toAddr = "0x" + randomHex(40);
    if (action === "cex_deposit") {
      toName = cex[Math.floor(rnd() * cex.length)]; toKind = "entity"; toEtype = "cex";
    } else if (rnd() < 0.4) {
      const e = ARKHAM_ENTITIES[Math.floor(rnd() * ARKHAM_ENTITIES.length)];
      toName = e.name; toKind = "entity"; toEtype = e.type;
    } else {
      toName = shortAddr(toAddr); toKind = "wallet"; toEtype = null;
    }
    amount *= 0.8 + rnd() * 0.17;
    hops.push({ action, toName, toKind, toEtype, toAddr, asset, amount, value: amount * (priceOf(asset) || 0), fromNet: curNet, toNet });
    curNet = toNet;
  }
  return hops;
}

function buildTraceHtml(w: Wallet): string {
  const net0 = networkById(w.networks[0]);
  const hops = buildTrace(w);
  const source = `
    <div data-wallet-addr="${escAttr(w.address)}" class="flex items-center gap-3 bg-panel2 border border-edge rounded-xl p-3 cursor-pointer hover:border-accent transition">
      <span class="w-3 h-3 rounded-full shrink-0" style="background:${WALLET_HEX}"></span>
      <div class="min-w-0">
        <div class="text-[11px] text-slate-400">${t("tracer_source")}</div>
        <div class="font-medium truncate">${w.label || shortAddr(w.address)}</div>
        <div class="font-mono text-[11px] text-slate-500 truncate">${shortAddr(w.address)} · <span class="${net0.color}">${net0.name}</span></div>
      </div>
    </div>`;

  const steps = hops.map((h, i) => {
    const am = ACTION_META[h.action];
    const fromNet = networkById(h.fromNet), toNet = networkById(h.toNet);
    const chain = h.action === "bridge"
      ? `<span class="${fromNet.color}">${fromNet.name}</span> → <span class="${toNet.color}">${toNet.name}</span>`
      : `<span class="${toNet.color}">${toNet.name}</span>`;
    const color = h.toKind === "wallet" ? WALLET_HEX : ENTITY_HEX[h.toEtype ?? "unknown"];
    const badge = h.toKind === "entity" && h.toEtype ? `<span class="px-1.5 py-0.5 rounded ${ENTITY_TYPE_CLS[h.toEtype]} text-[10px]">${t(`entity_${h.toEtype}`)}</span>` : "";
    return `
      <div class="relative pl-7">
        <div class="absolute left-[9px] top-0 ${i === hops.length - 1 ? "h-4" : "bottom-0"} w-px bg-edge"></div>
        <div class="absolute left-1 top-4 w-3 h-3 rounded-full ring-2 ring-panel" style="background:${color}"></div>
        <div class="pb-4 pt-1.5">
          <div class="text-[11px] ${am.cls} flex items-center gap-1.5">${am.icon} ${t(am.key)} · ${chain}</div>
          <div class="flex items-center justify-between gap-2 mt-0.5">
            <span class="font-medium truncate ${h.toAddr ? "cursor-pointer hover:text-accent" : ""}"${h.toAddr ? ` data-wallet-addr="${escAttr(h.toAddr)}"` : ""}>${h.toName} ${badge}</span>
            <span class="text-xs text-slate-400 shrink-0">${fmtNum(h.amount, 3)} ${h.asset}${h.value ? ` · ${fmtUsd(h.value)}` : ""}</span>
          </div>
          ${h.toAddr ? `<div class="flex gap-2"><button data-wallet-addr="${escAttr(h.toAddr)}" class="text-[11px] text-accent hover:underline">📊 ${t("pf_open")}</button><a href="${arkhamLink(h.toAddr)}" target="_blank" rel="noopener" class="text-[11px] text-purple-400 hover:underline">🔍 Arkham →</a></div>` : ""}
        </div>
      </div>`;
  }).join("");

  return `${source}<div class="mt-4">${steps}</div><div class="text-[11px] text-slate-500">${t("tracer_hint")}</div>`;
}

function populateTracerWalletSelect(): void {
  const sel = $<HTMLSelectElement>("#tracer-wallet-select");
  sel.innerHTML = state.wallets.map((w) => `<option value="${w.id}">${w.label || shortAddr(w.address)}</option>`).join("");
  if (tracerWid) sel.value = tracerWid;
}

export function openTracer(wid?: string): void {
  const w = wid ? state.wallets.find((x) => x.id === wid) : state.wallets[0];
  if (!w) return;
  tracerWid = w.id;
  populateTracerWalletSelect();
  $("#tracer-body").innerHTML = buildTraceHtml(w);
  const modal = $("#tracer-modal");
  modal.classList.remove("hidden"); modal.classList.add("flex");
}

export function closeTracer(): void {
  tracerWid = null;
  const modal = $("#tracer-modal");
  modal.classList.add("hidden"); modal.classList.remove("flex");
}


/** Перемалювати трасування під поточний гаманець (напр., після зміни мови). */
export function refreshTracer(): void {
  if (tracerWid) openTracer(tracerWid);
}
