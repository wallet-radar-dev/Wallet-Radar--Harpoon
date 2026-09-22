import type { Network } from "./types";

export const NETWORKS: Network[] = [
  { id: "eth", name: "Ethereum", symbol: "ETH", color: "text-indigo-400", explorer: "https://etherscan.io" },
  { id: "arbitrum", name: "Arbitrum", symbol: "ETH", color: "text-sky-400", explorer: "https://arbiscan.io" },
  { id: "base", name: "Base", symbol: "ETH", color: "text-blue-400", explorer: "https://basescan.org" },
  { id: "polygon", name: "Polygon", symbol: "MATIC", color: "text-purple-400", explorer: "https://polygonscan.com" },
  { id: "optimism", name: "Optimism", symbol: "ETH", color: "text-red-400", explorer: "https://optimistic.etherscan.io" },
  { id: "bnb", name: "BNB Chain (BSC)", symbol: "BNB", color: "text-yellow-400", explorer: "https://bscscan.com" },
  { id: "solana", name: "Solana", symbol: "SOL", color: "text-green-400", explorer: "https://solscan.io" },
  { id: "bitcoin", name: "Bitcoin", symbol: "BTC", color: "text-orange-400", explorer: "https://mempool.space" },
  { id: "tron", name: "Tron", symbol: "TRX", color: "text-rose-400", explorer: "https://tronscan.org" },
  { id: "dogecoin", name: "Dogecoin", symbol: "DOGE", color: "text-amber-400", explorer: "https://blockchair.com/dogecoin" },
  { id: "sonic", name: "Sonic", symbol: "S", color: "text-cyan-400", explorer: "https://sonicscan.org" },
  { id: "mantle", name: "Mantle", symbol: "MNT", color: "text-emerald-400", explorer: "https://mantlescan.xyz" },
  { id: "avalanche", name: "Avalanche", symbol: "AVAX", color: "text-red-500", explorer: "https://snowtrace.io" },
  { id: "flare", name: "Flare", symbol: "FLR", color: "text-pink-400", explorer: "https://flarescan.com" },
];

export const networkById = (id: string): Network =>
  NETWORKS.find((n) => n.id === id) ?? NETWORKS[0];

export function explorerAddr(net: Network, addr: string): string {
  if (net.id === "solana") return `${net.explorer}/account/${addr}`;
  if (net.id === "tron") return `${net.explorer}/#/address/${addr}`;
  return `${net.explorer}/address/${addr}`;
}

export function explorerTx(net: Network, hash: string): string {
  if (net.id === "tron") return `${net.explorer}/#/transaction/${hash}`;
  if (net.id === "dogecoin") return `${net.explorer}/transaction/${hash}`;
  return `${net.explorer}/tx/${hash}`;
}

export const EVM_CHAINS = ["eth", "arbitrum", "base", "polygon", "optimism", "bnb", "sonic", "mantle", "avalanche", "flare"];

/** Ймовірні мережі для адреси за її форматом. */
export function chainsForAddress(addr: string): string[] {
  if (addr.startsWith("0x")) return EVM_CHAINS;
  if (/^T[1-9A-HJ-NP-Za-km-z]{20,}$/.test(addr)) return ["tron"];
  if (/^D[1-9A-HJ-NP-Za-km-z]{25,}$/.test(addr)) return ["dogecoin"];
  if (/^(bc1|[13])[a-zA-HJ-NP-Z0-9]{20,}$/.test(addr)) return ["bitcoin"];
  return ["solana"];
}
