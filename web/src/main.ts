/**
 * Wallet Radar — клієнтська логіка (TypeScript).
 *
 * Компілюється в app.js (його й завантажує index.html).
 * Демо-режим: події генеруються локально. Real-time = connectWebSocket() замість startDemoFeed().
 * Підтримка 5 мов інтерфейсу (i18n) — див. TR + t().
 */

import type {
  Lang, Holding, Wallet, EventKind, EntityType,
  BlockchainEvent, WireEvent,
} from "./types";

import { t, lang, setLang } from "./i18n";
import { networkById } from "./networks";
import { state, byNewestFirst, insertEventSorted } from "./state";
import { $, uid, shortAddr, randomHex } from "./util";
import { priceOf, recomputeValues, updatePriceIndicator, fetchPrices, onPricesUpdated } from "./prices";
import { demoArkhamEntity } from "./attribution";
import { getJSON, postJSON, delJSON, fetchEvents, normalizeEvent } from "./api";
import { hydrateFromCache, onBalancesUpdated, requestBalance, retryBalance } from "./wallet-balances";
import { renderNetworkCheckboxes, selectedNetworks, renderStats, renderWallets, prependEvent, rebuildFeed } from "./render";
import { setupDialogFocusManagement } from "./dialog";
import { toast } from "./ui";
import { BACKEND } from "./env";
import { loadClarity } from "./clarity";
import { onSearchInput, closeSearch } from "./search";
import { loadUniverse } from "./universe";
import { loadGroupAssets } from "./group-assets";
import { openAssetHolders, closeAssetHolders, refreshAssetHolders } from "./holders";
import { openTracer, closeTracer, refreshTracer } from "./tracer";
import { openEntityProfile, closeEntityProfile, refreshEntity } from "./entity";
import { openAlerts, closeAlerts, renderAlerts, checkAlerts, behChangeText, addAlertRule, onBehaviorClick, updateAlertBadge } from "./alerts";
import { openDashboard, closeDashboard, renderDashboard, addDashWidget } from "./dashboard";
import { openPortfolio, openPortfolioFor, openWalletAddress, closePortfolio, syntheticWallet, getPortfolioWallet, refreshPortfolio, loadWalletReport } from "./portfolio";
import { nav } from "./nav";
import { openClusters } from "./clusters";
import { openTrackRecord } from "./trackrecord";
import { openBulkCheck } from "./check";
import { openExitPressure } from "./exitpressure";
import { initFreshness } from "./freshness";
import { openEventModal, openGroupAlert, closeEventModal } from "./event-modal";
import {
  openGraph, closeGraph, openGraphForWallet, openSmartMoney,
  loadCapitalGroups, openMoneyTraceGraph, loadGroupStats, refreshNodeOnUpdate, refreshGraphLang,
  openNetworkGraph, openGroupFlowGraph, checkGroupLink, checkGroupCoordination,
  clearFocus, focusWallet, removeGraphWallet, closeNodeDetail, setupGraphInteractions, cgNetworkAt,
} from "./graph";
import {
  openConcentration, closeConcentration, runConcentration, openPortfolioStrategy,
  closePortfolioStrategy, askTokenAI, loadAssetGroups, loadHolderAssets,
  exportConcentration, exportConcentrationCsv,
} from "./concentration";

/* ---------- Композиційний корінь: крос-модульні дії (nav) ---------- */

// Портфель (Arkham-картка) закриває всі інші оверлеї перед відкриттям — але не
// знає про них. Реалізацію постачає композиційний корінь; закривачі фіче-модулів
// імпортовані вище, closeGraph/closeEventModal — локальні (нижче в цьому файлі).
nav.closeOverlays = (): void => {
  document.getElementById("cluster-modal")?.classList.add("hidden");
  document.getElementById("sm-modal")?.classList.add("hidden");
  closeConcentration(); closeAssetHolders(); closeGraph(); closeTracer();
  closeDashboard(); closeEntityProfile(); closeEventModal(); closeAlerts();
};
nav.openWallet = openWalletAddress;
nav.groupStats = loadGroupStats;
nav.removeWallet = removeWallet;

/* ---------- Живі ціни: каскад перерендеру фіч ---------- */

// Модуль prices.ts не знає про фічі; тут композиційний корінь підписує каскад
// перерендеру, який виконується після кожного оновлення цін (fetchPrices).
onPricesUpdated(() => {
  renderWallets();
  renderStats();
  refreshPortfolio();
  refreshAssetHolders();
  if (!$("#dash-modal").classList.contains("hidden")) renderDashboard();
  refreshNodeOnUpdate();
});

// Баланси живуть у wallet-balances.ts: ліниво (лише видимі гаманці), не більше 3 запитів одночасно,
// з чесними станами. Тут — тільки підписка на оновлення; сам модуль про рендер не знає.
onBalancesUpdated(() => {
  renderWallets();
  renderStats();
});

/* ---------- Застосування перекладів до статичних елементів ---------- */

function applyI18n(): void {
  document.documentElement.lang = lang;
  document.title = t("doc_title");
  document.querySelectorAll<HTMLElement>("[data-i18n]").forEach((el) => {
    el.textContent = t(el.dataset.i18n as string);
  });
  document.querySelectorAll<HTMLInputElement>("[data-i18n-ph]").forEach((el) => {
    el.placeholder = t(el.dataset.i18nPh as string);
  });
  document.querySelectorAll<HTMLElement>("[data-i18n-title]").forEach((el) => {
    el.title = t(el.dataset.i18nTitle as string);
  });
  document.querySelectorAll<HTMLElement>("[data-i18n-aria]").forEach((el) => {
    el.setAttribute("aria-label", t(el.dataset.i18nAria as string));
  });
  const ct = document.querySelector<HTMLElement>("#conn-text");
  if (ct) ct.textContent = t(connLive ? "conn_live" : "conn_demo");
}


/* ---------- Дії ---------- */

function addWallet(address: string, networks: string[], label: string): void {
  if (READ_ONLY) { toast("👁 Режим перегляду", "Додавання гаманців недоступне у демо", "border-yellow-400"); return; }
  const holdings: Holding[] = []; // без синтетики — реальний баланс підтягнемо нижче
  const w: Wallet = { id: uid(), address, networks, label: label || undefined, holdings, valueUsd: 0 };
  state.wallets.unshift(w);
  renderWallets();
  renderStats();
  requestBalance(w.id); // щойно доданий гаманець — вантажимо баланс одразу, не чекаючи, поки він потрапить у поле зору
  if (BACKEND) {
    void postJSON("/api/wallets", { id: w.id, address, networks, label, holdings }).catch(() => {});
  }
  const netText = networks.length === 1 ? networkById(networks[0]).name : `${networks.length} ${t("networks_word")}`;
  toast(t("toast_added_title"), `${netText} · ${shortAddr(address)}`, "border-good");
}

function removeWallet(id: string): void {
  if (READ_ONLY) { toast("👁 Режим перегляду", "Видалення недоступне у демо", "border-yellow-400"); return; }
  state.wallets = state.wallets.filter((w) => w.id !== id);
  renderWallets();
  renderStats();
  if (BACKEND) void delJSON(`/api/wallets/${id}`).catch(() => {});
}

/* ---------- Зміна мови ---------- */

function setLanguage(l: Lang): void {
  setLang(l);
  localStorage.setItem("lang", l);
  applyI18n();
  updatePriceIndicator();
  renderWallets();
  renderStats();
  rebuildFeed();
  closeEventModal();
  refreshPortfolio();
  refreshAssetHolders();
  refreshTracer();
  refreshEntity();
  if (!$("#alerts-modal").classList.contains("hidden")) renderAlerts();
  if (!$("#dash-modal").classList.contains("hidden")) renderDashboard();
  // динамічні панелі, які рендеряться раз — перемальовуємо під нову мову
  const sm = document.getElementById("sm-modal");
  if (sm && !sm.classList.contains("hidden")) void openSmartMoney();
  if (!$("#conc-modal").classList.contains("hidden")) void runConcentration();
  void loadCapitalGroups();
  refreshGraphLang();
}

/* ---------- ДЕМО-стрічка (заглушка real-time) ---------- */

function randomEvent(): BlockchainEvent | null {
  if (state.wallets.length === 0) return null;
  const w = state.wallets[Math.floor(Math.random() * state.wallets.length)];
  const netId = w.networks[Math.floor(Math.random() * w.networks.length)];
  const net = networkById(netId);
  const kinds: EventKind[] = ["in", "out", "swap", "approval"];
  const kind = kinds[Math.floor(Math.random() * kinds.length)];
  const amount = +(Math.random() * 12).toFixed(4);
  return {
    id: uid(),
    walletId: w.id,
    network: netId,
    kind,
    asset: net.symbol,
    amount,
    valueUsd: Math.max(1, Math.round(amount * priceOf(net.symbol))),
    counterparty: "0x" + randomHex(40),
    counterpartyEntity: demoArkhamEntity(),
    timestamp: Date.now(),
    txHash: "0x" + randomHex(64),
    blockNumber: 18_000_000 + Math.floor(Math.random() * 5_000_000),
    feeUsd: +(Math.random() * 12).toFixed(2),
    status: "confirmed",
  };
}

function pushEvent(ev: BlockchainEvent): void {
  // Жива подія НЕ завжди найновіша: поллер доливає бекліг гаманця, тож у /ws цілком може прилетіти
  // транзакція годинної давнини. Раніше вона просто ставала зверху стрічки — звідси й стрибки часу.
  // Тепер подія лягає на своє місце; анімацію «нове зверху» лишаємо тільки тому, що зверху й є.
  const idx = insertEventSorted(ev);
  if (state.events.length > 200) state.events.pop();
  state.eventsToday += 1;
  if (idx === 0) prependEvent(ev);
  else rebuildFeed(); // подія «в середину» — дешевше перемалювати 60 рядків, ніж вставляти вручну
  renderStats();
  if (ev.kind as string !== "group") checkAlerts(ev);
}

/** Засів історичними подіями: усі гаманці одразу пов'язані через спільні сутності. */
function seedDemo(n: number): void {
  for (let i = 0; i < n; i++) {
    const ev = randomEvent();
    if (!ev) continue;
    ev.timestamp = Date.now() - (n - i) * 60_000;
    state.events.unshift(ev);
    state.eventsToday += 1;
  }
  if (state.events.length > 200) state.events.length = 200;
  rebuildFeed();
  renderStats();
}

function startDemoFeed(): void {
  const tick = (): void => {
    const ev = randomEvent();
    if (ev) pushEvent(ev);
    window.setTimeout(tick, 2500 + Math.random() * 4000);
  };
  window.setTimeout(tick, 1500);
}

/* ---------- Підключення до бекенду (real-time) ---------- */

let connLive = false;

// РЕЖИМ ДОСТУПУ: демо-перегляд (READ_ONLY) ховає керування змінами; адмін — повний доступ.
let READ_ONLY = false;
async function applyAccessMode(): Promise<void> {
  if (!BACKEND) return;
  try {
    const cfg = await getJSON<{ readOnly?: boolean }>("/api/config");
    READ_ONLY = !!cfg.readOnly;
    if (READ_ONLY) {
      document.body.classList.add("read-only");
      const cnt = document.getElementById("wallet-count");
      if (cnt && cnt.parentElement && !document.querySelector(".ro-badge")) {
        const b = document.createElement("span");
        b.className = "ro-badge"; b.textContent = "👁 Режим перегляду";
        cnt.parentElement.insertBefore(b, cnt);
      }
    }
  } catch { /* ignore */ }
}

function connectWebSocket(): void {
  const proto = location.protocol === "https:" ? "wss" : "ws";
  const ws = new WebSocket(`${proto}://${location.host}/ws`);
  ws.onopen = () => { connLive = true; $("#conn-text").textContent = t("conn_live"); };
  ws.onmessage = (m: MessageEvent) => {
    const msg = JSON.parse(m.data as string);
    if (msg.type === "event") {
      pushEvent(normalizeEvent(msg.event as WireEvent));
    } else if (msg.type === "init" && Array.isArray(msg.events)) {
      state.events = (msg.events as WireEvent[]).map(normalizeEvent).sort(byNewestFirst);
      state.eventsToday = state.events.length;
      rebuildFeed();
      renderStats();
    } else if (msg.type === "behavior" && Array.isArray(msg.changes)) {
      const who = (msg.label as string) || shortAddr(msg.address as string);
      for (const c of msg.changes as Array<{ type: string; from?: string; to?: string }>) {
        state.alertLog.unshift({ ruleId: "behavior", ts: (msg.ts as number) || Date.now(), text: `${who}: ${behChangeText(c)}` });
        state.alertCount += 1;
      }
      if (state.alertLog.length > 60) state.alertLog.length = 60;
      updateAlertBadge();
      toast(t("beh_alert_toast"), who, "border-yellow-400");
      if (!$("#alerts-modal").classList.contains("hidden")) renderAlerts();
    } else if (msg.type === "alpha") {
      const toks = (msg.tokens as string[] || []).join(", ");
      const who = shortAddr(msg.address as string);
      state.alertLog.unshift({ ruleId: "alpha", ts: (msg.ts as number) || Date.now(), text: `🟢 ${who} (${msg.viaAsset}) → ${t("alpha_new_token")}: ${toks}` });
      state.alertCount += 1;
      if (state.alertLog.length > 60) state.alertLog.length = 60;
      updateAlertBadge();
      toast(`🟢 ${t("alpha_toast")}`, `${who} → ${toks}`, "border-good");
      if (!$("#alerts-modal").classList.contains("hidden")) renderAlerts();
    }
  };
  ws.onclose = () => { connLive = false; $("#conn-text").textContent = t("conn_reconnect"); window.setTimeout(connectWebSocket, 2000); };
}

async function startData(): Promise<void> {
  if (BACKEND) {
    // Демо-дані — це ВИГАДКА. Вмикати їх можна рівно тоді, коли бекенда справді немає (file://
    // або гаманці не завантажились). Раніше цей try обіймав і завантаження, і рендер, тож БУДЬ-ЯКА
    // помилка рендера ковталась як «сервер недоступний»: застосунок мовчки підмінював реальні дані
    // вигаданими, не піднімав WebSocket і не оновлював баланси. Саме так один `null` у
    // counterpartyEntity поклав живий режим — і жодного слова в консоль. Тепер межа проходить тут:
    // не завантажилось → демо; завантажилось, але щось упало при рендері → кричимо, а не брешемо.
    let loaded = false;
    try {
      state.wallets = await getJSON<Wallet[]>("/api/wallets");
      // Баланси — з готового серверного кешу, ДО першого рендера: інакше спостерігач замовив би
      // наживо те, що вже пораховано, і ми повернулись би до 507 обходів по 15 мереж.
      await hydrateFromCache();
      try {
        state.events = await fetchEvents();
        state.eventsToday = state.events.length;
      } catch { /* події необов'язкові */ }
      loaded = true;
    } catch (e) {
      console.error("[startData] бекенд не відповів — переходжу в демо-режим", e);
    }

    if (loaded) {
      try {
        recomputeValues();
        renderWallets();
        renderStats();
        rebuildFeed();
      } catch (e) {
        // Показуємо збій, а не ховаємо його за демо-даними: краще порожня стрічка й гучна скарга,
        // ніж правдоподібна підробка, яку не відрізнити від реальних подій.
        console.error("[startData] помилка рендера реальних даних", e);
        toast("⚠ Помилка інтерфейсу", "Частина даних не відмалювалась — деталі в консолі", "border-bad");
      }
      // Жива стрічка не залежить від того, чи впав рендер: підключаємось у будь-якому разі.
      connectWebSocket();
      // Баланси вже прийшли з кешу (hydrateFromCache вище). Тих, кого в кеші не було, добере
      // спостерігач, начеплений у renderWallets(), — коли гаманець доїде до екрана. Залпу по всіх
      // 507 не робимо НІКОЛИ: саме він забивав усі 6 з'єднань браузера на години й не давав
      // завантажитись навіть сусідній вкладці.
      return;
    }
  }
  // Демо-режим (file:// або бекенд недоступний)
  seedDemo(18);
  startDemoFeed();
}

/* ---------- Старт ---------- */

function setAllChecks(checked: boolean): void {
  document.querySelectorAll<HTMLInputElement>(".net-check").forEach((c) => (c.checked = checked));
}

function init(): void {
  applyI18n();
  void applyAccessMode();
  void loadClarity();
  recomputeValues();
  updatePriceIndicator();
  renderNetworkCheckboxes();
  renderWallets();
  renderStats();
  void fetchPrices();
  window.setInterval(() => void fetchPrices(), 60_000);

  const langSel = $<HTMLSelectElement>("#lang-select");
  langSel.value = lang;
  langSel.addEventListener("change", () => setLanguage(langSel.value as Lang));

  $("#net-all").addEventListener("click", () => setAllChecks(true));
  $("#net-none").addEventListener("click", () => setAllChecks(false));

  $<HTMLFormElement>("#add-form").addEventListener("submit", (e: Event) => {
    e.preventDefault();
    const addr = $<HTMLInputElement>("#addr-input").value.trim();
    const nets = selectedNetworks();
    const label = $<HTMLInputElement>("#label-input").value.trim();
    if (!addr) { toast(t("err"), t("err_no_address"), "border-bad"); return; }
    if (nets.length === 0) { toast(t("err"), t("err_no_network"), "border-bad"); return; }
    addWallet(addr, nets, label);
    $<HTMLInputElement>("#addr-input").value = "";
    $<HTMLInputElement>("#label-input").value = "";
    setAllChecks(false);
  });

  $("#clear-feed").addEventListener("click", () => {
    state.events = [];
    rebuildFeed();
  });

  // Клік по події у стрічці → відкрити деталі
  $("#event-feed").addEventListener("click", (e: Event) => {
    const li = (e.target as HTMLElement).closest<HTMLElement>("li[data-ev]");
    if (!li?.dataset.ev) return;
    const ev = state.events.find((x) => x.id === li.dataset.ev);
    if (ev) { if (ev.kind as string === "group") openGroupAlert(ev); else openEventModal(ev); }
  });

  // Клік по гаманцю у списку → портфель (усі активи)
  $("#wallet-list").addEventListener("click", (e: Event) => {
    const el = e.target as HTMLElement;
    const del = el.closest<HTMLElement>("[data-del]");
    if (del) { removeWallet(del.dataset.del as string); return; }
    const retry = el.closest<HTMLElement>("[data-retry]");
    if (retry) { retryBalance(retry.dataset.retry as string); return; } // «⚠ баланс не завантажився» — клік повторює
    const li = el.closest<HTMLElement>("li[data-wallet]");
    if (li?.dataset.wallet) openPortfolio(li.dataset.wallet);
  });

  // Закриття модального вікна
  $("#modal-close").addEventListener("click", closeEventModal);
  $("#modal-backdrop").addEventListener("click", closeEventModal);
  $("#portfolio-close").addEventListener("click", closePortfolio);
  $("#portfolio-backdrop").addEventListener("click", closePortfolio);
  // Підсвічування сегмента діаграми розподілу при наведенні (на сегмент SVG або легенду)
  const donutHover = (e: Event, on: boolean): void => {
    const trg = (e.target as HTMLElement).closest<HTMLElement>("[data-donut-seg]");
    const root = (e.target as HTMLElement).closest<HTMLElement>("[data-donut-root]");
    if (!root) return;
    const segs = root.querySelectorAll<SVGPathElement>(".donut-seg");
    const center = root.querySelector<HTMLElement>("[data-donut-center]");
    const cl = root.querySelector<HTMLElement>("[data-donut-clabel]");
    const cv = root.querySelector<HTMLElement>("[data-donut-cval]");
    if (on && trg) {
      const idx = trg.dataset.donutSeg;
      const path = root.querySelector<SVGPathElement>(`path[data-donut-seg="${idx}"]`);
      segs.forEach((p) => p.classList.toggle("dim", p.dataset.donutSeg !== idx));
      path?.classList.add("pop");
      if (path && cl && cv) { cl.textContent = `${path.dataset.label} · ${path.dataset.pct}%`; cv.textContent = path.dataset.val || ""; }
    } else {
      segs.forEach((p) => p.classList.remove("dim", "pop"));
      if (center && cl && cv) { cl.textContent = center.dataset.clabel || ""; cv.textContent = center.dataset.cval || ""; }
    }
  };
  $("#portfolio-body").addEventListener("mouseover", (e) => donutHover(e, true));
  $("#portfolio-body").addEventListener("mouseout", (e) => donutHover(e, false));
  $("#asset-close").addEventListener("click", closeAssetHolders);
  $("#asset-backdrop").addEventListener("click", closeAssetHolders);
  // Клік по тримачу-гаманцю у вікні активу → його портфель
  $("#asset-body").addEventListener("click", (e: Event) => {
    const li = (e.target as HTMLElement).closest<HTMLElement>("[data-wallet]");
    if (li?.dataset.wallet) { closeAssetHolders(); openPortfolio(li.dataset.wallet); }
  });
  // Переходи з картки гаманця: граф / tracer / подія / сутність
  $("#portfolio-body").addEventListener("click", (e: Event) => {
    const el = e.target as HTMLElement;
    const mt = el.closest<HTMLElement>("[data-money-trace]");
    if (mt?.dataset.moneyTrace) { void openMoneyTraceGraph(mt.dataset.moneyTrace, getPortfolioWallet()?.label); return; }
    const rp = el.closest<HTMLElement>("[data-report]");
    if (rp?.dataset.report) { void loadWalletReport(rp.dataset.report); return; }
    const g = el.closest<HTMLElement>("[data-graph-wallet]");
    if (g?.dataset.graphWallet) { openGraphForWallet(g.dataset.graphWallet); return; }
    const tr = el.closest<HTMLElement>("[data-tracer-wallet]");
    if (tr?.dataset.tracerWallet) { closePortfolio(); openTracer(tr.dataset.tracerWallet); return; }
    const cl = el.closest<HTMLElement>("[data-cluster-wallet]");
    if (cl?.dataset.clusterWallet) { void openClusters(cl.dataset.clusterWallet); return; }
    const ent = el.closest<HTMLElement>("[data-entity-name]");
    if (ent?.dataset.entityName) { closePortfolio(); openEntityProfile(ent.dataset.entityName, ent.dataset.entityType as EntityType); return; }
    const evb = el.closest<HTMLElement>("[data-ev]");
    if (evb?.dataset.ev) { const x = state.events.find((z) => z.id === evb.dataset.ev); if (x) { closePortfolio(); if (x.kind as string === "group") openGroupAlert(x); else openEventModal(x); } }
  });

  // Перехід із картки події → профіль сутності контрагента
  $("#modal-body").addEventListener("click", (e: Event) => {
    const ent = (e.target as HTMLElement).closest<HTMLElement>("[data-entity-name]");
    if (ent?.dataset.entityName) { closeEventModal(); openEntityProfile(ent.dataset.entityName, ent.dataset.entityType as EntityType); }
  });
  // Tracer
  $("#tracer-btn").addEventListener("click", () => openTracer());
  $("#tracer-close").addEventListener("click", closeTracer);
  $("#tracer-backdrop").addEventListener("click", closeTracer);
  $("#tracer-wallet-select").addEventListener("change", (e: Event) => openTracer((e.target as HTMLSelectElement).value));

  // Концентрація власників
  $("#conc-btn").addEventListener("click", openConcentration);
  $("#clusters-btn").addEventListener("click", () => void openClusters());
  $("#sm-btn").addEventListener("click", () => void openSmartMoney());
  $("#strat-btn").addEventListener("click", () => void openPortfolioStrategy());
  $("#strat-close").addEventListener("click", closePortfolioStrategy);
  $("#strat-backdrop").addEventListener("click", closePortfolioStrategy);
  $("#cg-refresh").addEventListener("click", () => void loadCapitalGroups());
  $("#capital-groups").addEventListener("click", (e: Event) => {
    const el = e.target as HTMLElement;
    // розкриття групи: обсяг + статистика всіх гаманців
    const exp = el.closest<HTMLElement>("[data-group-expand]");
    if (exp?.dataset.groupExpand) { const cont = exp.closest<HTMLElement>(".rounded-lg")?.querySelector<HTMLElement>("[data-group-stats]"); if (cont) void loadGroupStats(exp.dataset.groupExpand, cont); return; }
    const ng = el.closest<HTMLElement>("[data-net-graph-cg]");
    if (ng?.dataset.netGraphCg) { const n = cgNetworkAt(Number(ng.dataset.netGraphCg)); if (n) openNetworkGraph(n); return; }
    const gfb = el.closest<HTMLElement>("[data-group-flow]");
    if (gfb?.dataset.groupFlow) { void openGroupFlowGraph(gfb.dataset.groupFlow); return; }
    const res = el.closest<HTMLElement>(".rounded-lg")?.querySelector<HTMLElement>("[data-cg-res]");
    if (!res) return;
    const link = el.closest<HTMLElement>("[data-cg-link]");
    if (link?.dataset.cgLink) { const [f, to] = link.dataset.cgLink.split("|"); void checkGroupLink(f, to, res); return; }
    const coord = el.closest<HTMLElement>("[data-cg-coord]");
    if (coord?.dataset.cgCoord) void checkGroupCoordination(coord.dataset.cgCoord, res);
  });
  void loadCapitalGroups();
  // Живе авто-оновлення панелі, поки фоновий forensics-воркер працює (без затирання відкритих результатів)
  setInterval(() => {
    if (document.hidden) return;
    const busy = !!document.querySelector("#capital-groups [data-cg-res]:not(:empty)");
    if (busy) return;
    void loadCapitalGroups();
  }, 60000);
  // Глобально: клік по будь-якій адресі гаманця (звідусіль) → Arkham-картка
  document.addEventListener("click", (e: Event) => {
    const el = (e.target as HTMLElement).closest<HTMLElement>("[data-wallet-addr]");
    if (el?.dataset.walletAddr) { e.preventDefault(); openWalletAddress(el.dataset.walletAddr, el.dataset.walletLabel || ""); }
  });
  $("#conc-close").addEventListener("click", closeConcentration);
  $("#conc-backdrop").addEventListener("click", closeConcentration);
  $("#conc-analyze").addEventListener("click", () => void runConcentration());
  $<HTMLInputElement>("#conc-input").addEventListener("keydown", (e: KeyboardEvent) => { if (e.key === "Enter") void runConcentration(); });
  $("#conc-body").addEventListener("keydown", (e: KeyboardEvent) => { if (e.key === "Enter" && (e.target as HTMLElement).id === "conc-ai-input") void askTokenAI(); });
  $("#conc-body").addEventListener("click", (e: Event) => {
    const el = e.target as HTMLElement;
    if (el.closest("#conc-export-csv")) { exportConcentrationCsv(); return; }
    if (el.closest("#conc-export")) { exportConcentration(); return; }
    if (el.closest("#conc-groups")) { void loadAssetGroups(); return; }
    if (el.closest("#conc-ai-ask")) { void askTokenAI(); return; }
    const aiq = el.closest<HTMLElement>(".conc-ai-q");
    if (aiq?.dataset.q) { const inp = $<HTMLInputElement>("#conc-ai-input"); inp.value = aiq.dataset.q; void askTokenAI(); return; }
    const as = el.closest<HTMLElement>("[data-assets]");
    if (as?.dataset.assets) { void loadHolderAssets(as.dataset.assets); return; }
    const tk = el.closest<HTMLElement>("[data-track-addr]");
    if (tk?.dataset.trackAddr) { addWallet(tk.dataset.trackAddr, ["eth"], `🐋 ${tk.dataset.trackSym || ""}`.trim()); }
  });

  // Dashboard
  $("#dash-btn").addEventListener("click", openDashboard);
  $("#dash-close").addEventListener("click", closeDashboard);
  $("#dash-backdrop").addEventListener("click", closeDashboard);
  $("#dash-add").addEventListener("click", addDashWidget);
  $("#dash-grid").addEventListener("click", (e: Event) => {
    const b = (e.target as HTMLElement).closest<HTMLElement>("[data-del-widget]");
    if (b?.dataset.delWidget) { state.dashboard = state.dashboard.filter((w) => w.id !== b.dataset.delWidget); renderDashboard(); }
  });

  // Entity-профіль
  $("#entity-close").addEventListener("click", closeEntityProfile);
  $("#entity-backdrop").addEventListener("click", closeEntityProfile);
  $("#entity-body").addEventListener("click", (e: Event) => {
    const b = (e.target as HTMLElement).closest<HTMLElement>("[data-wallet]");
    if (b?.dataset.wallet) { closeEntityProfile(); openPortfolio(b.dataset.wallet); }
  });

  // Alerts
  updateAlertBadge();
  void initFreshness();
  $("#exitpress-btn").addEventListener("click", () => void openExitPressure());
  $("#bulkcheck-btn").addEventListener("click", () => void openBulkCheck());
  $("#trackrec-btn").addEventListener("click", () => void openTrackRecord());
  $("#alerts-btn").addEventListener("click", openAlerts);
  $("#alerts-close").addEventListener("click", closeAlerts);
  $("#alerts-backdrop").addEventListener("click", closeAlerts);
  $("#alert-add").addEventListener("click", addAlertRule);
  $("#alerts-behavior").addEventListener("click", (e: Event) => void onBehaviorClick(e));
  $("#alerts-rules").addEventListener("click", (e: Event) => {
    const b = (e.target as HTMLElement).closest<HTMLElement>("[data-del-alert]");
    if (b?.dataset.delAlert) { state.alerts = state.alerts.filter((r) => r.id !== b.dataset.delAlert); renderAlerts(); }
  });

  document.addEventListener("keydown", (e: KeyboardEvent) => {
    if (e.key === "Escape") { closeEventModal(); closeGraph(); closePortfolio(); closeAssetHolders(); closeTracer(); closeAlerts(); closeEntityProfile(); closeDashboard(); closeConcentration(); closeSearch(); }
  });
  setupDialogFocusManagement();

  // Пошук (адреса гаманця / актив)
  const searchInput = $<HTMLInputElement>("#search-input");
  searchInput.addEventListener("input", () => onSearchInput(searchInput.value));
  searchInput.addEventListener("focus", () => { if (searchInput.value) onSearchInput(searchInput.value); });
  searchInput.addEventListener("blur", () => window.setTimeout(() => $("#search-results").classList.add("hidden"), 200));
  searchInput.addEventListener("keydown", (e: KeyboardEvent) => {
    if (e.key === "Escape") closeSearch();
    if (e.key === "Enter") { e.preventDefault(); $<HTMLElement>("#search-results [data-s-type]")?.click(); }
  });
  $("#search-results").addEventListener("click", (e: Event) => {
    const el = (e.target as HTMLElement).closest<HTMLElement>("[data-s-type]");
    if (!el) return;
    const type = el.dataset.sType, val = el.dataset.sVal as string;
    closeSearch();
    if (type === "asset") {
      const c = el.dataset.sContract;
      if (c) { openConcentration(); $<HTMLInputElement>("#conc-input").value = c; void runConcentration(); }
      else {
        const cg = el.dataset.sCg ?? null, nm = el.dataset.sName ?? null;
        void (async () => {
          if (cg) {
            try {
              const r = await getJSON<{ contract?: string }>("/api/cg-asset", { id: cg });
              if (r.contract && /^0x[0-9a-fA-F]{40}$/.test(r.contract)) {
                openConcentration(); $<HTMLInputElement>("#conc-input").value = r.contract; void runConcentration(); return;
              }
            } catch { /* fall through to holders */ }
          }
          void openAssetHolders(val, cg, nm);
        })();
      }
    }
    else if (type === "wallet") openPortfolio(val);
    else if (type === "address") openPortfolioFor(syntheticWallet(val));
  });

  // Граф зв'язків
  $("#graph-btn").addEventListener("click", openGraph);
  $("#graph-close").addEventListener("click", closeGraph);
  $("#graph-showall").addEventListener("click", clearFocus);
  $("#graph-wallet-select").addEventListener("change", (e: Event) => {
    const v = (e.target as HTMLSelectElement).value;
    document.getElementById("graph-wallet-del")?.classList.toggle("hidden", !v);
    if (v) focusWallet(v, false); else clearFocus();
  });
  $("#graph-wallet-del").addEventListener("click", removeGraphWallet);
  $("#graph-detail").addEventListener("click", (e: Event) => {
    const el = e.target as HTMLElement;
    if (el.closest("#graph-detail-close")) { closeNodeDetail(); return; }
    const ep = el.closest<HTMLElement>("[data-entity-name]");
    if (ep?.dataset.entityName) { openEntityProfile(ep.dataset.entityName, ep.dataset.entityType as EntityType); return; }
    const pb = el.closest<HTMLElement>("[data-portfolio-wallet]");
    if (pb?.dataset.portfolioWallet) { openPortfolio(pb.dataset.portfolioWallet); return; }
    const fb = el.closest<HTMLElement>("[data-focus-wallet]");
    if (fb?.dataset.focusWallet) focusWallet(fb.dataset.focusWallet);
  });
  setupGraphInteractions();

  void loadUniverse();
  void startData();
}

document.addEventListener("DOMContentLoaded", () => {
  init();
  // активи груп вантажаться паралельно: не блокують основний старт
  loadGroupAssets();
});
