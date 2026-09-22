/**
 * Типізований клієнт HTTP-API бекенда.
 *
 * Єдина точка мережевого вводу-виводу: замість ~60 розкиданих `fetch()` із
 * ~120 нетипізованими `as {…}`-кастами — один helper `getJSON<T>()`, що
 * повертає перевірений тип, і structured `ApiError`, який не «ковтається»
 * мовчки на межі. Типи відповідей додаються поendpoint-но в міру того, як
 * модулі мігрують сюди (co-located з функцією-обгорткою).
 */

import type { ArkhamEntity, BlockchainEvent, WireEvent } from "./types";
import { byNewestFirst } from "./state";

/** Помилка запиту до API — несе HTTP-статус і шлях; `status === 0` = мережевий збій. */
export class ApiError extends Error {
  constructor(
    readonly status: number,
    readonly endpoint: string,
    message?: string,
  ) {
    super(message ?? `API ${endpoint} → HTTP ${status}`);
    this.name = "ApiError";
  }
}

type QueryValue = string | number | boolean | undefined | null;

function queryString(params?: Record<string, QueryValue>): string {
  if (!params) return "";
  const pairs = Object.entries(params)
    .filter((e): e is [string, string | number | boolean] => e[1] !== undefined && e[1] !== null && e[1] !== "")
    .map(([k, v]) => `${encodeURIComponent(k)}=${encodeURIComponent(String(v))}`);
  return pairs.length ? "?" + pairs.join("&") : "";
}

/**
 * GET, що повертає типізований JSON. Кидає `ApiError` на не-2xx, мережевий
 * збій або невалідний JSON — виклик сам вирішує, як реагувати (fallback,
 * тост, ігнор), замість того щоб кожен catch мовчки ковтав будь-яку помилку.
 */
export async function getJSON<T>(path: string, params?: Record<string, QueryValue>): Promise<T> {
  const url = path + queryString(params);
  let res: Response;
  const ctrl = new AbortController();
  const to = window.setTimeout(() => ctrl.abort(), 60000);
  try {
    res = await fetch(url, { signal: ctrl.signal });
  } catch (e) {
    throw new ApiError(0, path, ctrl.signal.aborted ? "timeout" : (e instanceof Error ? e.message : "network error"));
  } finally {
    window.clearTimeout(to);
  }
  if (!res.ok) throw new ApiError(res.status, path, await serverErrorMessage(res));
  try {
    return (await res.json()) as T;
  } catch {
    throw new ApiError(res.status, path, "invalid JSON");
  }
}

/** Дістати змістовне `{ error }` із тіла не-2xx відповіді (бекенд повертає його для більшості endpoint'ів). */
async function serverErrorMessage(res: Response): Promise<string | undefined> {
  try {
    const body = (await res.json()) as { error?: unknown };
    if (typeof body?.error === "string" && body.error) return body.error;
  } catch { /* тіло не JSON — лишаємо стандартне повідомлення ApiError */ }
  return undefined;
}

/** POST JSON-тіла. Кидає ApiError на не-2xx. Повертає розпарсену відповідь або undefined. */
export async function postJSON<T = void>(path: string, body: unknown): Promise<T> {
  let res: Response;
  try {
    res = await fetch(path, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) });
  } catch (e) {
    throw new ApiError(0, path, e instanceof Error ? e.message : "network error");
  }
  if (!res.ok) throw new ApiError(res.status, path, await serverErrorMessage(res));
  const text = await res.text();
  return (text ? JSON.parse(text) : undefined) as T;
}

/** DELETE. Кидає ApiError на не-2xx. */
export async function delJSON(path: string): Promise<void> {
  let res: Response;
  try {
    res = await fetch(path, { method: "DELETE" });
  } catch (e) {
    throw new ApiError(0, path, e instanceof Error ? e.message : "network error");
  }
  if (!res.ok) throw new ApiError(res.status, path, await serverErrorMessage(res));
}

/* ───────────────────────── Типізовані endpoint'и ─────────────────────────
   Кожен модуль, мігруючи сюди, додає свій блок: інтерфейс(и) відповіді +
   тонку функцію-обгортку над getJSON. */

// ── /api/cg-price ── котирування CoinGecko: cgId → { usd }
export type CgPriceResponse = Record<string, { usd?: number } | undefined>;
export const fetchCgPrices = (ids: string): Promise<CgPriceResponse> =>
  getJSON<CgPriceResponse>("/api/cg-price", { ids });

// ── /api/wallet-assets ── реальний ончейн-баланс EVM-гаманця
export interface WalletAssetsResponse {
  totalUsd: number;
  tokens: unknown[]; // деталізується, коли портфельний модуль мігрує сюди
}
export const fetchWalletAssets = (address: string): Promise<WalletAssetsResponse> =>
  getJSON<WalletAssetsResponse>("/api/wallet-assets", { address });

// ── /api/wallet-balances ── ГОТОВИЙ кеш балансів усіх гаманців (наповнює серверний воркер
// balances_tick, цикл 2 год). Один запит замість 507 живих обходів по 15 мереж кожен.
export interface CachedBalance {
  totalUsd: number;
  tokens: number;
  chains: number;
  source: string;
  updatedAt: number; // epoch ms — коли цей баланс востаннє ВДАЛОСЯ дізнатись
}
export interface WalletBalancesResponse {
  generated: number;
  count: number; // скільки гаманців мають ВІДОМИЙ баланс
  tracked: number; // скільки їх узагалі під наглядом — різниця й є покриття кешу
  totalUsd: number;
  wallets: Record<string, CachedBalance | undefined>; // ключ — адреса в нижньому регістрі
}
export const fetchWalletBalances = (): Promise<WalletBalancesResponse> =>
  getJSON<WalletBalancesResponse>("/api/wallet-balances");

// ── події стрічки ──────────────────────────────────────────────────────────────
// Єдиний вхід сирої події в застосунок: і HTTP (/api/events), і WebSocket (/ws) проходять тут.

/** Невідомий контрагент — це обʼєкт, а не діра (контракт feed.js:163). */
const UNKNOWN_ENTITY: ArkhamEntity = { name: "", type: "unknown" };

/**
 * Приводить сиру подію з дроту до внутрішнього контракту.
 *
 * Історія одного `null`: сервер слав counterpartyEntity: null, рендер стрічки читав `.type`,
 * падав на першій же події — а `catch` у startData приймав це за «бекенд недоступний» і клав
 * застосунок у демо-режим із вигаданими подіями. Сервер полагоджено, але дріт — чужа територія:
 * нормалізуємо на вході, щоб один кривий запис ніколи більше не вирішував долю всього UI.
 */
export function normalizeEvent(ev: WireEvent): BlockchainEvent {
  return { ...ev, counterpartyEntity: ev.counterpartyEntity ?? UNKNOWN_ENTITY };
}

/** Події з /api/events — нормалізовані й упорядковані від найновішої (сховище віддає їх у порядку
 *  прибуття, див. byNewestFirst). */
export const fetchEvents = async (): Promise<BlockchainEvent[]> =>
  (await getJSON<WireEvent[]>("/api/events")).map(normalizeEvent).sort(byNewestFirst);
