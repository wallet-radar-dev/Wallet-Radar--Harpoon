/* ---------- CLARITY Act: ймовірний регуляторний статус активу ---------- */

import { getJSON } from "./api";
import { BACKEND } from "./env";
import { clarityShortL } from "./i18n";
import { escAttr } from "./ui";

export type ClarityMeta = { short: string; label: string; color: string; note: string };

let CLARITY: { map: Record<string, string>; meta: Record<string, ClarityMeta> } | null = null;

export async function loadClarity(): Promise<void> {
  if (!BACKEND || CLARITY) return;
  try { CLARITY = await getJSON<{ map: Record<string, string>; meta: Record<string, ClarityMeta> }>("/api/clarity-tags"); } catch { /* ignore */ }
}

/** true, якщо для символу є ЯВНИЙ clarity-тег (не запасний «unknown»). */
export const hasClarityTag = (sym: string): boolean => !!(CLARITY && CLARITY.map[sym.toUpperCase()]);

export function clarityOf(sym: string): (ClarityMeta & { status: string }) | null {
  if (!CLARITY) return null;
  const s = String(sym || "").toUpperCase().trim();
  let status = CLARITY.map[s];
  if (!status) status = (/USD|EUR/.test(s) && s.length >= 4) ? "stablecoin" : "unknown";
  const m = CLARITY.meta[status];
  return m ? { status, ...m } : null;
}

// Маленький бейдж зі статусом CLARITY (кольорова крапка + короткий підпис, tooltip із поясненням).
export function clarityBadge(sym: string, withText = true): string {
  const c = clarityOf(sym);
  if (!c) return "";
  const short = clarityShortL(c.status);
  const text = withText ? `<span style="color:${c.color}">${short}</span>` : "";
  return `<span class="cl-badge" title="CLARITY Act — ${escAttr(short)}: ${escAttr(c.note)}" style="display:inline-flex;align-items:center;gap:4px;margin-left:6px;font-size:10px;padding:1px 6px;border-radius:10px;background:#0e1626;border:1px solid #27324a;vertical-align:middle"><span style="width:6px;height:6px;border-radius:50%;background:${c.color}"></span>${text}</span>`;
}

// Компактна легенда кольорів CLARITY (декодує крапки/бейджі).
export function clarityLegendHtml(): string {
  if (!CLARITY) return "";
  const order = ["commodity", "commodity-likely", "stablecoin", "rwa", "exchange", "contested"];
  const items = order.map((s) => { const m = CLARITY!.meta[s]; return m ? `<span style="display:inline-flex;align-items:center;gap:3px" title="${escAttr(m.note)}"><span style="width:6px;height:6px;border-radius:50%;background:${m.color}"></span>${clarityShortL(s)}</span>` : ""; }).join("");
  return `<div class="flex flex-wrap gap-x-3 gap-y-1 mt-2 pt-2 border-t border-edge/50 text-[10px] text-slate-400"><span class="text-slate-500">CLARITY Act:</span>${items}</div>`;
}
