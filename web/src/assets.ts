/* ---------- Активи: кольори бренду + довідник для пошуку ---------- */

const ASSET_HEX: Record<string, string> = {
  ETH: "#627eea", BTC: "#f7931a", WBTC: "#f09242", SOL: "#14f195", BNB: "#f3ba2f",
  USDT: "#26a17b", USDC: "#2775ca", DAI: "#f5ac37", ARB: "#28a0f0", OP: "#ff0420",
  LINK: "#2a5ada", UNI: "#ff007a", AAVE: "#b6509e", PEPE: "#3d8c40", MATIC: "#8247e5",
  TRX: "#eb0029", DOGE: "#c2a633", AVAX: "#e84142", MNT: "#65b3ae", FLR: "#e6195e", S: "#22d3ee",
};

/** Колір активу: фірмовий, або детермінований із символу. */
export function assetColor(sym: string): string {
  if (ASSET_HEX[sym]) return ASSET_HEX[sym];
  let h = 0;
  for (const c of sym) h = (h * 31 + c.charCodeAt(0)) >>> 0;
  return `hsl(${h % 360} 65% 55%)`;
}

// Довідник активів для пошуку (локальні підказки до онлайн-результатів CoinGecko).
export const ASSET_DIR: { sym: string; name: string }[] = [
  { sym: "ETH", name: "Ethereum" }, { sym: "BTC", name: "Bitcoin" }, { sym: "WBTC", name: "Wrapped Bitcoin" },
  { sym: "SOL", name: "Solana" }, { sym: "BNB", name: "BNB" }, { sym: "USDT", name: "Tether" },
  { sym: "USDC", name: "USD Coin" }, { sym: "DAI", name: "Dai" }, { sym: "ARB", name: "Arbitrum" },
  { sym: "OP", name: "Optimism" }, { sym: "LINK", name: "Chainlink" }, { sym: "UNI", name: "Uniswap" },
  { sym: "AAVE", name: "Aave" }, { sym: "PEPE", name: "Pepe" }, { sym: "MATIC", name: "Polygon" },
  { sym: "TRX", name: "Tron" }, { sym: "DOGE", name: "Dogecoin" }, { sym: "AVAX", name: "Avalanche" },
  { sym: "MNT", name: "Mantle" }, { sym: "FLR", name: "Flare" },
];
