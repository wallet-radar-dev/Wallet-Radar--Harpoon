import type { Wallet, BlockchainEvent, AlertRule, AlertHit, DashWidget } from "./types";

/* Central mutable app state (an object, so cross-module mutation works). */
export const state: { wallets: Wallet[]; events: BlockchainEvent[]; eventsToday: number; alerts: AlertRule[]; alertLog: AlertHit[]; alertCount: number; dashboard: DashWidget[] } = {
  alerts: [{ id: "a1", scope: "any", target: "", kind: "any", minUsd: 50_000 }],
  alertLog: [],
  alertCount: 0,
  dashboard: [
    { id: "d1", type: "total", target: "" },
    { id: "d2", type: "price", target: "BTC" },
    { id: "d3", type: "price", target: "ETH" },
    { id: "d4", type: "wallet", target: "w2" },
    { id: "d5", type: "holders", target: "ETH" },
    { id: "d6", type: "price", target: "SOL" },
  ],
  wallets: [
    { id: "w1", address: "0x742d35Cc6634C0532925a3b844Bc454e4438f44e", networks: ["eth", "arbitrum", "base"], label: "кит / OTC-деск", holdings: [{ asset: "ETH", amount: 412.5 }, { asset: "WBTC", amount: 8.3 }, { asset: "USDT", amount: 1_250_000 }, { asset: "ARB", amount: 320_000 }, { asset: "LINK", amount: 45_000 }], valueUsd: 0 },
    { id: "w2", address: "0xAb5801a7D398351b8bE11C439e05C5B3259aeC9B", networks: ["eth", "arbitrum"], label: "Vitalik Buterin", holdings: [{ asset: "ETH", amount: 1325.0 }, { asset: "USDC", amount: 250_000 }, { asset: "UNI", amount: 40_000 }, { asset: "AAVE", amount: 1_800 }, { asset: "PEPE", amount: 1_200_000_000 }], valueUsd: 0 },
    { id: "w3", address: "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM", networks: ["solana"], label: "Solana кит", holdings: [{ asset: "SOL", amount: 18_450 }, { asset: "USDC", amount: 300_000 }], valueUsd: 0 },
  ],
  events: [],
  eventsToday: 0,
};

/**
 * Стрічка подій показується від найновішої до найстарішої — але сервер зберігає їх у порядку
 * ПРИБУТТЯ, а не за часом транзакції (store.rs add_event вставляє на початок; Node так само робить
 * unshift). Поллер обходить гаманці по черзі й доливає їхній бекліг, тож мітки часу приходять
 * перемішані: 21:44 → 21:48 → 21:47 → 22:08. Сортуємо на нашому боці, бо порядок у сховищі —
 * це журнал прибуття, і він має право бути таким.
 *
 * Стабільність: при рівних мітках зберігаємо взаємний порядок (події одного блоку не стрибають).
 */
export const byNewestFirst = (a: BlockchainEvent, b: BlockchainEvent): number => b.timestamp - a.timestamp;

/** Кладе подію на її місце за часом і повертає індекс, куди вона стала (0 = найновіша). */
export function insertEventSorted(ev: BlockchainEvent): number {
  const at = state.events.findIndex((e) => e.timestamp <= ev.timestamp);
  const idx = at === -1 ? state.events.length : at;
  state.events.splice(idx, 0, ev);
  return idx;
}
