/**
 * Реальні ончейн-баланси гаманців — ліниво, з обмеженою паралельністю, з видимими помилками.
 *
 * ЩО БУЛО. `refreshAllWalletValues()` робив `Promise.all` по ВСІХ гаманцях — 507 запитів
 * `/api/wallet-assets` одним залпом. Кожен такий запит — це живий обхід 15 мереж на сервері, ~190с
 * (заміряно). Браузер тримає 6 з'єднань на хост, тож виходило 6 × 190с, і черга з 500 запитів
 * розсмоктувалася б ГОДИНАМИ. Наслідки: баланси фактично не завантажувались (9 із 507 за 20 хв);
 * усі 6 з'єднань були зайняті годинами, тож сусідня вкладка застосунку не могла завантажити навіть
 * app.js; сервер весь цей час дарма молотив зовнішні API. А `catch {}` ковтав помилки, тож гаманець
 * навіки лишався з написом «баланс завантажується…» — хоча ніхто вже нічого не вантажив.
 *
 * ЩО ТЕПЕР — ДВА ДЖЕРЕЛА, у такому порядку:
 *
 *  1. КЕШ (`hydrateFromCache`, /api/wallet-balances). Баланси всіх гаманців рахує серверний воркер
 *     (balances_tick, цикл 2 год) — у нього є час на обхід 15 мереж × 507 гаманців, а в браузера
 *     нема. Один запит на старті дає готові суми, і «Сумарна вартість» нарешті рахується по ВСІХ
 *     гаманцях, а не по тих кількох, повз які ви проскролили.
 *  2. ЖИВИЙ ДОБІР (нижче). Для гаманця, якого в кеші немає (щойно доданий — воркер ще не дійшов) або
 *     чий запис протух, баланс запитується наживо, коли гаманець реально показався на екрані
 *     (IntersectionObserver), не більше MAX_INFLIGHT запитів одночасно.
 *
 * Кеш не є обов'язковим: якщо /api/wallet-balances не відповів (воркер ще жодного разу не
 * відпрацював), застосунок просто лишається на живому доборі — повільно, але правдиво.
 *
 * Кожен гаманець має ЧЕСНИЙ стан: не запитували / у черзі / вантажимо / готово / не вдалося. Стан
 * «не вдалося» видно в UI, і його можна повторити кліком — мовчазної вічної «загрузки» більше немає.
 *
 * Оновлення транслюється підписникам (той самий seam, що й onPricesUpdated у prices.ts), щоб цей
 * модуль не знав про рендер.
 */
import type { Wallet } from "./types";
import { state } from "./state";
import { fetchWalletAssets, fetchWalletBalances } from "./api";
import { BACKEND } from "./env";

export type BalanceStatus = "idle" | "queued" | "loading" | "done" | "error";

/**
 * Браузер дає 6 одночасних з'єднань на хост. Беремо половину: решта потрібна самому застосунку
 * (події, ціни, групи, WebSocket-и модалок) — інакше баланси знову з'їдять увесь канал, як раніше.
 */
const MAX_INFLIGHT = 3;

const status = new Map<string, BalanceStatus>();
const queue: string[] = [];
let inflight = 0;

export const balanceStatusOf = (id: string): BalanceStatus => status.get(id) ?? "idle";

const listeners: Array<() => void> = [];
export const onBalancesUpdated = (fn: () => void): void => void listeners.push(fn);

// Відповіді приходять пачками; перемальовувати список на кожну — марна робота. Склеюємо в один кадр.
let scheduled = false;
function notify(): void {
  if (scheduled) return;
  scheduled = true;
  requestAnimationFrame(() => {
    scheduled = false;
    for (const fn of listeners) fn();
  });
}

/**
 * Наскільки старим може бути запис у кеші, щоб ми ще вважали гаманець «готовим».
 *
 * Воркер обходить усіх раз на 2 години, тож запис старший за 12 годин означає, що гаманець не
 * оновлювався шість циклів поспіль — його джерело системно мовчить. Число таке ще показуємо (це
 * реальний виміряний баланс, просто вчорашній), але гаманець лишається в стані "idle", і
 * спостерігач перезапитає його наживо, коли той доїде до екрана.
 */
const STALE_MS = 12 * 3600 * 1000;

/**
 * Заповнює баланси з готового серверного кешу. Викликається ОДИН раз на старті, до першого рендера.
 *
 * Це та сама робота, яку фронт колись намагався зробити сам 507 запитами: тут вона коштує один
 * запит, бо її вже зробив той, у кого є час. Гаманці, яких у кеші немає, нічого не втрачають — вони
 * підуть звичним лінивим шляхом.
 */
export async function hydrateFromCache(): Promise<void> {
  if (!BACKEND) return;
  let cache;
  try {
    cache = await fetchWalletBalances();
  } catch (e) {
    // Кеш — прискорювач, а не умова роботи: мовчки лишаємось на живому доборі. Але в консоль
    // кажемо чесно, інакше «чомусь усе повільно» буде нічим не пояснити.
    console.error("[balances] кеш недоступний — лишаюсь на живому доборі по одному", e);
    return;
  }

  const now = Date.now();
  let known = 0;
  let stale = 0;
  for (const w of state.wallets) {
    const e = cache.wallets?.[w.address.toLowerCase()];
    if (!e || typeof e.totalUsd !== "number") continue; // немає в кеші → лінивий шлях
    w.realUsd = e.totalUsd;
    w.realTokens = e.tokens;
    w.valueUsd = e.totalUsd;
    known += 1;
    if (now - (e.updatedAt ?? 0) < STALE_MS) status.set(w.id, "done");
    else stale += 1; // значення лишили, стан — ні: хай спостерігач оновить його наживо
  }

  if (known > 0) notify();
  console.info(
    `[balances] з кешу: ${known}/${state.wallets.length} гаманців` +
      (stale > 0 ? `, з них ${stale} протухлих — перезапитаю наживо` : "") +
      (known < state.wallets.length ? `, решту доберу ліниво` : ""),
  );
}

/**
 * Ставить гаманець у чергу на завантаження балансу (ідемпотентно).
 *
 * Адресу не фільтруємо за форматом: /api/wallet-assets вміє не лише EVM, а й BTC/Tron/Solana
 * (nonevm.rs), і старий код дарма їх пропускав — їхній «баланс» був синтетичною оцінкою з holdings.
 * Незрозумілу адресу сервер відкине 400-м, і гаманець чесно отримає стан "error".
 */
export function requestBalance(id: string): void {
  const w = state.wallets.find((x) => x.id === id);
  if (!w || !BACKEND) return;
  const s = balanceStatusOf(id);
  if (s === "queued" || s === "loading" || s === "done") return; // "error" → повтор дозволено
  status.set(id, "queued");
  queue.push(id);
  pump();
}

/** Повторити після помилки (клік по гаманцю з ⚠). */
export function retryBalance(id: string): void {
  if (balanceStatusOf(id) === "error") {
    status.delete(id);
    requestBalance(id);
  }
}

/**
 * Гаманець зник з екрана, а його запит ще не стартував — знімаємо з черги.
 *
 * Без цього прокрутка списку до кінця ставила б у чергу всі 507 гаманців: повільніше за старий залп,
 * але так само дарма — сервер годинами рахував би баланси для гаманців, повз які ви просто проїхали.
 * Запит, що ВЖЕ пішов, не чіпаємо: відповідь усе одно прийде, а обірвати її дорожче, ніж дочекатись.
 */
export function cancelQueued(id: string): void {
  if (balanceStatusOf(id) !== "queued") return;
  const at = queue.indexOf(id);
  if (at !== -1) queue.splice(at, 1);
  status.delete(id);
  notify();
}

function pump(): void {
  while (inflight < MAX_INFLIGHT && queue.length > 0) {
    const id = queue.shift() as string;
    const w = state.wallets.find((x) => x.id === id);
    if (!w) continue;
    inflight += 1;
    status.set(id, "loading");
    notify();
    void load(w).finally(() => {
      inflight -= 1;
      notify();
      pump();
    });
  }
}

async function load(w: Wallet): Promise<void> {
  try {
    const d = await fetchWalletAssets(w.address);
    if (typeof d.totalUsd !== "number") {
      status.set(w.id, "error"); // відповідь без суми — це збій, а не «нуль»
      return;
    }
    w.realUsd = d.totalUsd;
    w.realTokens = Array.isArray(d.tokens) ? d.tokens.length : undefined;
    w.valueUsd = d.totalUsd;
    status.set(w.id, "done");
  } catch {
    status.set(w.id, "error"); // мовчазне ковтання помилки — те, через що «завантажується…» жило вічно
  }
}

// ── лінивий тригер: гаманець показався на екрані ──────────────────────────────
let io: IntersectionObserver | null = null;

/**
 * Перечіпляє спостерігач на поточні <li> списку. Викликається ПІСЛЯ кожного renderWallets():
 * список перемальовується цілком (innerHTML), тож старі вузли зникають разом зі спостереженням.
 * rootMargin дає запас — баланс починає вантажитись трохи раніше, ніж гаманець доїде до екрана.
 */
export function observeWalletList(): void {
  const list = document.querySelector("#wallet-list");
  if (!list) return;
  io ??= new IntersectionObserver(
    (entries) => {
      for (const e of entries) {
        const id = (e.target as HTMLElement).dataset.wallet;
        if (!id) continue;
        if (e.isIntersecting) requestBalance(id);
        else cancelQueued(id); // проїхали повз — не витрачаємо на нього чергу
      }
    },
    { rootMargin: "300px" },
  );
  io.disconnect();
  list.querySelectorAll<HTMLElement>("li[data-wallet]").forEach((li) => io?.observe(li));
}
