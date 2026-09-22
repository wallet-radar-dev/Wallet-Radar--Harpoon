/* ---------- Arkham Intelligence: атрибуція контрагентів ---------- */

import type { EntityType, ArkhamEntity } from "./types";
import { t } from "./i18n";

export const ENTITY_TYPE_CLS: Record<EntityType, string> = {
  cex: "text-yellow-400 bg-yellow-400/10",
  fund: "text-purple-400 bg-purple-400/10",
  defi: "text-sky-400 bg-sky-400/10",
  issuer: "text-emerald-400 bg-emerald-400/10",
  individual: "text-pink-400 bg-pink-400/10",
  unknown: "text-slate-400 bg-slate-500/10",
};

// Кольори вузлів графа (hex) — сутності за типом + гаманець.
export const ENTITY_HEX: Record<EntityType, string> = {
  cex: "#eab308", fund: "#a855f7", defi: "#38bdf8", issuer: "#10b981", individual: "#f472b6", unknown: "#64748b",
};
export const WALLET_HEX = "#3b82f6";

// Демо-пул. У РОБОЧІЙ версії — поле `arkhamEntity` з відповіді
// GET https://api.arkhamintelligence.com/intelligence/address/{address}/all (header API-Key).
export const ARKHAM_ENTITIES: ArkhamEntity[] = [
  { name: "Binance", type: "cex" },
  { name: "Coinbase", type: "cex" },
  { name: "OKX", type: "cex" },
  { name: "Kraken", type: "cex" },
  { name: "Wintermute", type: "fund" },
  { name: "Jump Trading", type: "fund" },
  { name: "Tether Treasury", type: "issuer" },
  { name: "Uniswap Router", type: "defi" },
  { name: "Justin Sun", type: "individual" },
];

export function demoArkhamEntity(): ArkhamEntity {
  if (Math.random() < 0.4) return { name: "", type: "unknown" };
  return ARKHAM_ENTITIES[Math.floor(Math.random() * ARKHAM_ENTITIES.length)];
}

export const arkhamLink = (addr: string): string =>
  `https://intel.arkm.com/explorer/address/${addr}`;

export const entityBadge = (e: ArkhamEntity): string => {
  const cls = ENTITY_TYPE_CLS[e.type];
  const typeLabel = t(`entity_${e.type}`);
  if (e.type === "unknown") return `<span class="px-2 py-0.5 rounded-full text-xs ${cls}">${typeLabel}</span>`;
  return `<span class="px-2 py-0.5 rounded-full text-xs ${cls}">${e.name}</span> <span class="text-xs text-slate-500">${typeLabel}</span>`;
};
