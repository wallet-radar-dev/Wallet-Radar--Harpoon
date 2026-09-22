/* ---------- Наш універс активів (whale-assets): для пошуку + картки активу ----------
   Завантажується один раз і тримається в памʼяті: усі активи, що є в програмі,
   знаходяться в пошуку МИТТЄВО (без CoinGecko), а картка активу бере звідси
   ранг/сектор/концентрацію. */

import { getJSON } from "./api";

export interface UAsset {
  symbol: string;
  name: string;
  contract: string;
  rank: number | null;
  sector: string;
  top10pct: number | null;
  top50pct: number | null;
}

let UNIVERSE: UAsset[] = [];
let byContract = new Map<string, UAsset>();
let bySymbol = new Map<string, UAsset>();
let loaded = false;
let loading: Promise<void> | null = null;

export function loadUniverse(): Promise<void> {
  if (loaded) return Promise.resolve();
  if (loading) return loading;
  loading = (async () => {
    try {
      const d = await getJSON<{ assets: Array<{ symbol?: string; name?: string; contract?: string; rank?: number; sector?: string; top10pct?: number; top50pct?: number }> }>("/api/whale-assets");
      UNIVERSE = (d.assets || []).map((a) => ({
        symbol: String(a.symbol || "").toUpperCase(),
        name: a.name || a.symbol || "",
        contract: String(a.contract || "").toLowerCase(),
        rank: a.rank ?? null,
        sector: a.sector || "other",
        top10pct: a.top10pct ?? null,
        top50pct: a.top50pct ?? null,
      })).filter((a) => a.symbol);
      byContract = new Map(UNIVERSE.filter((a) => a.contract).map((a) => [a.contract, a]));
      bySymbol = new Map(UNIVERSE.map((a) => [a.symbol, a]));
      loaded = true;
    } catch { /* лишаємо порожнім — пошук впаде на CoinGecko */ }
  })();
  return loading;
}

export function universeByContract(c: string | null | undefined): UAsset | undefined {
  return c ? byContract.get(c.toLowerCase()) : undefined;
}
export function universeBySymbol(s: string | null | undefined): UAsset | undefined {
  return s ? bySymbol.get(s.toUpperCase()) : undefined;
}

/** Пошук по нашому універсу: збіг за символом або назвою, відсортовано за рангом. */
export function searchUniverse(q: string, limit = 12): UAsset[] {
  const ql = q.trim().toLowerCase();
  if (!ql) return [];
  const hits = UNIVERSE.filter((a) => a.symbol.toLowerCase().includes(ql) || a.name.toLowerCase().includes(ql));
  hits.sort((a, b) => {
    // точний збіг символу — першим; далі за рангом (nulls останні)
    const ax = a.symbol.toLowerCase() === ql ? 0 : 1;
    const bx = b.symbol.toLowerCase() === ql ? 0 : 1;
    if (ax !== bx) return ax - bx;
    return (a.rank ?? 1e9) - (b.rank ?? 1e9);
  });
  return hits.slice(0, limit);
}

const SECTOR_EMOJI: Record<string, string> = { depin: "📡", memecoin: "🐸", defi: "🏦", rwa: "🏛", other: "◽" };
export function sectorEmoji(sec: string): string { return SECTOR_EMOJI[sec] || "◽"; }
