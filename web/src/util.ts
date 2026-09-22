import { lang } from "./i18n";

/* DOM + formatting helpers shared across the app. */
export const $ = <T extends HTMLElement = HTMLElement>(sel: string): T =>
  document.querySelector(sel) as T;

export const uid = (): string =>
  Date.now().toString(36) + Math.floor(Math.random() * 1e6).toString(36);

// Детермінований ГВЧ із рядка-сіда (стабільні графіки/холдинги для однієї адреси)
export function seededRand(seed: string): () => number {
  let h = 2166136261;
  for (const c of seed) h = Math.imul(h ^ c.charCodeAt(0), 16777619);
  return () => { h += 0x6d2b79f5; let t = h; t = Math.imul(t ^ (t >>> 15), t | 1); t ^= t + Math.imul(t ^ (t >>> 7), t | 61); return ((t ^ (t >>> 14)) >>> 0) / 4294967296; };
}

export const shortAddr = (a: string): string =>
  a.length > 14 ? `${a.slice(0, 6)}…${a.slice(-4)}` : a;

export const localeOf = (): string => (lang === "en" ? "en-US" : lang === "de" ? "de-DE" : lang === "fr" ? "fr-FR" : lang === "ru" ? "ru-RU" : "uk-UA");

export const fmtUsd = (n: number): string =>
  "$" + n.toLocaleString(localeOf(), { maximumFractionDigits: 0 });

export const fmtNum = (n: number, max = 4): string =>
  n.toLocaleString(localeOf(), { maximumFractionDigits: max });

// Ціна активу: більше знаків для дешевих токенів (щоб не показувати $0/$1).
export const fmtPrice = (n: number): string =>
  "$" + n.toLocaleString(localeOf(), { maximumFractionDigits: n >= 1 ? 2 : n >= 0.01 ? 4 : 8 });

export const fmtTime = (ts: number): string =>
  new Date(ts).toLocaleTimeString(localeOf(), { hour: "2-digit", minute: "2-digit", second: "2-digit" });

export const fmtDateTime = (ts: number): string =>
  new Date(ts).toLocaleString(localeOf(), { dateStyle: "medium", timeStyle: "medium" });

export const randomHex = (len: number): string =>
  Array.from({ length: len }, () => Math.floor(Math.random() * 16).toString(16)).join("");
