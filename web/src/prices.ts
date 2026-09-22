/* ---------- Живі ціни (CoinGecko) — доменний модуль ---------- */

import { state } from "./state";
import { t } from "./i18n";
import { localeOf } from "./util";
import { fetchCgPrices } from "./api";

// Резервні ціни (станом на калібрування) — використовуються, якщо запит не вдався.
const FALLBACK_PRICES: Record<string, number> = {
  ETH: 1664.16, BTC: 62423, SOL: 68.94, BNB: 576.23, MATIC: 0.5,
  TRX: 0.328675, DOGE: 0.078646, AVAX: 6.41, MNT: 0.514165, FLR: 0.00702151, S: 0.5,
  // токени
  USDT: 1, USDC: 1, DAI: 1, WBTC: 62000, ARB: 0.45, OP: 1.1, LINK: 7.2, UNI: 4.8, AAVE: 92, PEPE: 0.0000082,
};
// символ активу → id у CoinGecko
const CG_IDS: Record<string, string> = {
  ETH: "ethereum", BTC: "bitcoin", SOL: "solana", BNB: "binancecoin", MATIC: "matic-network",
  TRX: "tron", DOGE: "dogecoin", AVAX: "avalanche-2", MNT: "mantle", FLR: "flare-networks",
  USDT: "tether", USDC: "usd-coin", DAI: "dai", WBTC: "wrapped-bitcoin", ARB: "arbitrum",
  OP: "optimism", LINK: "chainlink", UNI: "uniswap", AAVE: "aave", PEPE: "pepe",
};

const prices: Record<string, number> = { ...FALLBACK_PRICES };
let pricesUpdated: number | null = null;
let pricesLive = false;

export const priceOf = (asset: string): number => prices[asset] ?? 0;
// Записати ціну в кеш (напр., ensurePrice підвантажує актив на вимогу).
export const setPrice = (asset: string, usd: number): void => { prices[asset] = usd; };
// id у CoinGecko для локально відомого символу (або null).
export const cgIdOf = (asset: string): string | null => CG_IDS[asset] ?? null;

export function recomputeValues(): void {
  for (const w of state.wallets) {
    w.valueUsd = typeof w.realUsd === "number" ? w.realUsd : w.holdings.reduce((s, h) => s + h.amount * priceOf(h.asset), 0);
  }
}

export function updatePriceIndicator(): void {
  const el = document.querySelector<HTMLElement>("#price-indicator");
  if (!el) return;
  const time = pricesUpdated
    ? new Date(pricesUpdated).toLocaleTimeString(localeOf(), { hour: "2-digit", minute: "2-digit" })
    : "—";
  el.innerHTML = `<span class="${pricesLive ? "text-good" : "text-yellow-400"}">●</span> ${t("prices_word")} ${time}${pricesLive ? "" : " · " + t("prices_offline")}`;
  el.title = t("prices_src");
}

// Observer-«шов»: модуль цін не знає про фічі. Композиційний корінь (main.ts)
// реєструє каскад перерендеру через onPricesUpdated(), розриваючи цикл залежностей.
let afterPrices: () => void = () => { /* реєструється в main */ };
export function onPricesUpdated(cb: () => void): void { afterPrices = cb; }

export async function fetchPrices(): Promise<void> {
  try {
    const ids = [...new Set(Object.values(CG_IDS))].join(",");
    const data = await fetchCgPrices(ids);
    for (const [sym, cgId] of Object.entries(CG_IDS)) {
      const usd = data[cgId]?.usd;
      if (typeof usd === "number") prices[sym] = usd;
    }
    pricesLive = true;
  } catch {
    pricesLive = false; // будь-яка помилка (ApiError/мережа) → лишаємось на резервних цінах
  }
  pricesUpdated = Date.now();
  recomputeValues();
  updatePriceIndicator();
  afterPrices();
}
