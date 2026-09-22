/* ---------- Alerts: правила сповіщень ---------- */

import type { BlockchainEvent, AlertRule, EventKind } from "./types";
import { t } from "./i18n";
import { $, uid, shortAddr, fmtUsd, fmtTime } from "./util";
import { state } from "./state";
import { toast, escAttr } from "./ui";
import { getJSON } from "./api";
import { networkById } from "./networks";
import { kindLabel } from "./render";


function alertMatches(ev: BlockchainEvent, r: AlertRule): boolean {
  if (ev.valueUsd < r.minUsd) return false;
  if (r.kind !== "any" && ev.kind !== r.kind) return false;
  if (r.scope === "wallet") {
    const tgt = r.target.toLowerCase();
    if (!tgt) return true;
    const w = state.wallets.find((x) => x.id === ev.walletId);
    return ev.walletId === r.target || !!(w && ((w.label || "").toLowerCase().includes(tgt) || w.address.toLowerCase().includes(tgt)));
  }
  if (r.scope === "asset") return !r.target || ev.asset.toLowerCase() === r.target.toLowerCase();
  return true;
}

export function updateAlertBadge(): void {
  const b = $("#alert-badge");
  if (state.alertCount > 0) { b.textContent = String(state.alertCount); b.classList.remove("hidden"); }
  else b.classList.add("hidden");
}

export function checkAlerts(ev: BlockchainEvent): void {
  for (const r of state.alerts) {
    if (!alertMatches(ev, r)) continue;
    const text = `${kindLabel(ev.kind)} · ${fmtUsd(ev.valueUsd)} · ${networkById(ev.network).name}`;
    state.alertLog.unshift({ ruleId: r.id, ts: ev.timestamp, text });
    if (state.alertLog.length > 60) state.alertLog.pop();
    state.alertCount += 1;
    updateAlertBadge();
    toast(`🔔 ${t("alerts_triggered")}`, text, "border-bad");
    if (!$("#alerts-modal").classList.contains("hidden")) renderAlerts();
    break; // одне спрацювання на подію
  }
}

const alertRuleText = (r: AlertRule): string => {
  const scope = r.scope === "any" ? t("alerts_any")
    : r.scope === "wallet" ? `${t("g_wallet_one")}: ${r.target || t("alerts_any")}`
    : `${t("search_assets")}: ${r.target || t("alerts_any")}`;
  const kind = r.kind === "any" ? t("alerts_any") : kindLabel(r.kind);
  return `${scope} · ${kind} · ≥ ${fmtUsd(r.minUsd)}`;
};

export function fillAlertSelects(): void {
  const sc = $<HTMLSelectElement>("#alert-scope");
  const pv = sc.value;
  sc.innerHTML = `<option value="any">${t("alerts_any")}</option><option value="wallet">${t("g_wallet_one")}</option><option value="asset">${t("search_assets")}</option>`;
  if (pv) sc.value = pv;
  const kd = $<HTMLSelectElement>("#alert-kind");
  const pk = kd.value;
  kd.innerHTML = `<option value="any">${t("alerts_any")}</option>` + (["in", "out", "swap", "approval"] as EventKind[]).map((k) => `<option value="${k}">${kindLabel(k)}</option>`).join("");
  if (pk) kd.value = pk;
}

export function renderAlerts(): void {
  fillAlertSelects();
  $("#alerts-rules").innerHTML = state.alerts.length
    ? state.alerts.map((r) => `
      <div class="flex items-center justify-between gap-2 bg-panel2 border border-edge rounded-lg px-3 py-2 text-sm">
        <span class="truncate">${alertRuleText(r)}</span>
        <button data-del-alert="${r.id}" class="text-slate-500 hover:text-bad text-lg leading-none shrink-0">×</button>
      </div>`).join("")
    : `<div class="text-xs text-slate-500">${t("alerts_empty")}</div>`;
  $("#alerts-log").innerHTML = state.alertLog.length
    ? state.alertLog.slice(0, 30).map((h) => `
      <div class="flex items-center justify-between gap-2 text-xs py-1.5 border-b border-edge/60">
        <span class="truncate">🔔 ${h.text}</span><span class="text-slate-500 shrink-0">${fmtTime(h.ts)}</span>
      </div>`).join("")
    : `<div class="text-xs text-slate-500">${t("alerts_no_hits")}</div>`;
  const bw = state.wallets.filter((w) => /^0x[0-9a-fA-F]{40}$/.test(w.address));
  $("#alerts-behavior").innerHTML = bw.length ? bw.map((w) => `
      <div class="bg-panel2 border border-edge rounded-lg px-3 py-2 text-sm">
        <div class="flex items-center justify-between gap-2">
          <span class="truncate">${w.label || shortAddr(w.address)}</span>
          <span class="flex gap-1.5 shrink-0">
            <button data-beh-arm="${escAttr(w.address)}" class="text-[11px] px-2 py-1 rounded bg-panel border border-edge hover:border-accent transition">${t("beh_arm")}</button>
            <button data-beh-check="${escAttr(w.address)}" class="text-[11px] px-2 py-1 rounded bg-accent/15 text-accent border border-accent/30 transition">${t("beh_check")}</button>
          </span>
        </div>
        <div data-beh-res="${escAttr(w.address)}" class="text-[11px] text-slate-500 mt-1"></div>
      </div>`).join("") : `<div class="text-xs text-slate-500">${t("alerts_empty")}</div>`;
  void loadAlphaFeed();
  void loadVolatilityWatch();
}

// Alpha-стрічка: «кит зайшов у новий токен» (із фонового воркера). Адреси клікабельні → портфель.
export async function loadAlphaFeed(): Promise<void> {
  const el = document.getElementById("alerts-alpha");
  if (!el) return;
  try {
    const d = await getJSON<{ feed: Array<{ address: string; tokens: string[]; viaAsset: string; share: number; ts: number }> }>("/api/alpha-feed");
    const feed = d.feed || [];
    el.innerHTML = feed.length ? feed.slice(0, 25).map((a) => `
      <div class="flex items-center justify-between gap-2 bg-good/5 border border-good/20 rounded-lg px-2.5 py-1.5 text-[11px]">
        <span class="truncate">🟢 <span data-wallet-addr="${escAttr(a.address)}" class="font-mono cursor-pointer hover:text-accent">${shortAddr(a.address)}</span>
          <span class="text-slate-500">(${a.viaAsset} ${a.share}%)</span> → <b class="text-good">${a.tokens.join(", ")}</b></span>
        <span class="text-slate-500 shrink-0">${fmtTime(a.ts)}</span>
      </div>`).join("") : `<div class="text-xs text-slate-500">${t("alpha_empty")}</div>`;
  } catch { el.innerHTML = `<div class="text-xs text-slate-500">${t("alpha_empty")}</div>`; }
}

// Волатильний watch: кити тяжко набирають USD -> навчена модель прогнозує великий ±рух (не напрям). /api/whale-volatility.
export async function loadVolatilityWatch(): Promise<void> {
  const el = document.getElementById("alerts-volatility");
  if (!el) return;
  try {
    const d = await getJSON<{ tokens: Array<{ chain: string; token: string; symbol: string; pBigMove: number; usdAccum7d: number }> }>("/api/whale-volatility");
    const toks = d.tokens || [];
    el.innerHTML = toks.length ? toks.slice(0, 12).map((x) => `
      <div class="flex items-center justify-between gap-2 bg-panel2 border border-edge rounded-lg px-2.5 py-1.5 text-[11px]">
        <span class="truncate">🎯 <b class="text-yellow-400">${x.symbol || x.token.slice(0, 8)}</b> <span class="text-slate-500">${x.chain}</span></span>
        <span class="text-slate-400 shrink-0">P=${Math.round(x.pBigMove * 100)}% · ${fmtUsd(x.usdAccum7d)}</span>
      </div>`).join("") : `<div class="text-xs text-slate-500">${t("vol_watch_empty")}</div>`;
  } catch { el.innerHTML = `<div class="text-xs text-slate-500">${t("vol_watch_empty")}</div>`; }
}

export function behChangeText(c: { type: string; from?: string; to?: string }): string {
  switch (c.type) {
    case "stance": return `${t("beh_stance_change")}: ${t("cl_" + c.from)} → ${t("cl_" + c.to)}`;
    case "dominant": return `${t("beh_focus_change")}: ${t("cl_cat_" + c.from)} → ${t("cl_cat_" + c.to)}`;
    case "sell_up": return t("beh_sell_up");
    case "buy_up": return t("beh_buy_up");
    case "netflow_flip": return t("beh_netflip");
    case "activity_spike": return t("beh_spike");
    default: return c.type;
  }
}

export async function onBehaviorClick(e: Event): Promise<void> {
  const el = e.target as HTMLElement;
  const arm = el.closest<HTMLElement>("[data-beh-arm]");
  const chk = el.closest<HTMLElement>("[data-beh-check]");
  const addr = arm?.dataset.behArm || chk?.dataset.behCheck;
  if (!addr) return;
  const res = document.querySelector<HTMLElement>(`[data-beh-res="${addr}"]`);
  if (res) res.textContent = t("beh_checking");
  try {
    const d = await getJSON<{ hasBaseline?: boolean; changes?: Array<{ type: string; from?: string; to?: string }> }>("/api/behavior-check", { address: addr, arm: arm ? 1 : undefined });
    if (!res) return;
    if (arm) { res.textContent = t("beh_armed"); return; }
    if (!d.hasBaseline) { res.textContent = t("beh_no_base"); return; }
    const changes = d.changes ?? [];
    if (!changes.length) { res.textContent = t("beh_nochange"); return; }
    const texts: string[] = changes.map((c) => behChangeText(c));
    res.innerHTML = texts.map((x) => `<div class="text-yellow-400">⚠ ${x}</div>`).join("");
    texts.forEach((x) => state.alertLog.unshift({ ruleId: "behavior", ts: Date.now(), text: `${shortAddr(addr)}: ${x}` }));
    $("#alerts-log").innerHTML = state.alertLog.slice(0, 30).map((h) => `<div class="flex items-center justify-between gap-2 text-xs py-1.5 border-b border-edge/60"><span class="truncate">🔔 ${h.text}</span><span class="text-slate-500 shrink-0">${fmtTime(h.ts)}</span></div>`).join("");
  } catch { if (res) res.textContent = t("conc_err"); }
}

export function addAlertRule(): void {
  const scope = $<HTMLSelectElement>("#alert-scope").value as AlertRule["scope"];
  const target = $<HTMLInputElement>("#alert-target").value.trim();
  const kind = $<HTMLSelectElement>("#alert-kind").value as AlertRule["kind"];
  const minUsd = Math.max(0, Number($<HTMLInputElement>("#alert-min").value) || 0);
  state.alerts.push({ id: uid(), scope, target, kind, minUsd });
  $<HTMLInputElement>("#alert-target").value = "";
  renderAlerts();
}

export function openAlerts(): void {
  renderAlerts();
  const m = $("#alerts-modal");
  m.classList.remove("hidden"); m.classList.add("flex");
}

export function closeAlerts(): void {
  const m = $("#alerts-modal");
  m.classList.add("hidden"); m.classList.remove("flex");
}

