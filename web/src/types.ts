/* Shared domain types for the Wallet Radar client. */

export type Lang = "uk" | "en" | "de" | "fr" | "ru";

export interface Network {
  id: string;
  name: string;
  symbol: string;
  color: string; // tailwind-клас тексту
  explorer: string; // базовий URL блокчейн-оглядача
}

export interface Holding {
  asset: string; // символ активу, напр. "ETH"
  amount: number;
}

export interface Wallet {
  id: string;
  address: string;
  networks: string[]; // один або кілька Network.id
  label?: string;
  holdings: Holding[];
  valueUsd: number; // обчислюється з holdings × живі ціни (recomputeValues)
  realUsd?: number; // РЕАЛЬНИЙ ончейн-баланс (із /api/wallet-assets) — пріоритетний над синтетичним
  realTokens?: number; // реальна к-сть активів
}

export type EventKind = "in" | "out" | "swap" | "approval";
export type EntityType = "cex" | "fund" | "defi" | "issuer" | "individual" | "unknown";

export interface ArkhamEntity {
  name: string;
  type: EntityType;
}

export interface BlockchainEvent {
  id: string;
  walletId: string;
  network: string;
  kind: EventKind;
  asset: string;
  amount: number;
  valueUsd: number;
  counterparty: string;
  counterpartyEntity: ArkhamEntity;
  timestamp: number;
  txHash: string;
  blockNumber: number;
  feeUsd: number;
  status: "confirmed" | "pending";
  // поля події «група капіталу» (kind === "group")
  category?: string;
  icon?: string;
  title?: string;
  body?: string;
  address?: string | null;
  members?: string[];
  assets?: string[];
  meta?: { long?: number; short?: number; dir?: string; top?: Array<{ symbol: string; usd: number }> } | null;
}

/**
 * Подія ТАК, ЯК ВОНА ПРИХОДИТЬ ДРОТОМ, — до нормалізації.
 *
 * `BlockchainEvent` вище обіцяє, що `counterpartyEntity` — завжди обʼєкт, і півтора десятка місць
 * (стрічка, граф, портфель, модалка) читають `.type` без перевірки. Rust-порт же слав туди `null`
 * для невідомого контрагента — і перший же такий запис клав рендер усієї стрічки, а застосунок
 * тихо сідав на демо-дані. Сервер полагоджено (feed.rs), але тип більше не вірить дроту на слово:
 * сирі дані мають форму WireEvent, і єдиний шлях із неї у BlockchainEvent — normalizeEvent().
 * Тепер це не домовленість, а вимога компілятора.
 */
export type WireEvent = Omit<BlockchainEvent, "counterpartyEntity"> & {
  counterpartyEntity?: ArkhamEntity | null;
};

export interface AlertRule {
  id: string;
  scope: "any" | "wallet" | "asset";
  target: string; // walletId або символ активу (для scope=any — порожньо)
  kind: "any" | EventKind;
  minUsd: number;
}

export interface AlertHit { ruleId: string; ts: number; text: string; }

export interface DashWidget { id: string; type: "total" | "price" | "wallet" | "holders"; target: string; }
