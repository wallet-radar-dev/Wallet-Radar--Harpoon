//! wr-dict — СПІЛЬНИЙ СЛОВНИК ДАНИХ (Aladdin-принцип: одна мова даних на всі модулі).
//! ЄДИНЕ місце визначень усіх понять — щоб цифри між ІНСТРУМЕНТАМИ не розходились.
//! На нього спираються і рушій `patterns`, і (поступово) старі біни backfill/nextaction.
//! Правило: жоден інструмент не хардкодить поріг/контракт/формулу локально — усе бере ЗВІДСИ.
//! Версіонування: DICT_VERSION росте при зміні визначень (відтворюваність вітрин).

// ── ПОРОГИ (єдині для всього рушія) ─────────────────────────────────────────
//
// Раніше це були `pub const`. Тепер — функції, що читають SQLite.
// Причина не в красі: константа, зашита в компіляцію, робить неможливим
// відповісти на питання «з якими порогами порахований ось цей звіт» —
// значення змінилось, а старі цифри лишились без пояснення.
//
// Ціна зміни: значення беруться з бази при першому зверненні. Немає бази —
// паніка з поясненням, а не мовчазний нуль (нуль тут = «поріг вимкнено»).
// Читається зі знімка в пам'яті, тож у гарячих циклах це HashMap-пошук.

macro_rules! from_db {
    ($(#[$m:meta])* $fname:ident, $key:literal, f64) => {
        $(#[$m])*
        pub fn $fname() -> f64 { wr_store::snapshot().num($key).unwrap_or_else(|e| panic!("{e}")) }
    };
    ($(#[$m:meta])* $fname:ident, $key:literal, $t:ty) => {
        $(#[$m])*
        pub fn $fname() -> $t { wr_store::snapshot().int($key).unwrap_or_else(|e| panic!("{e}")) as $t }
    };
}

/// Версія визначень. Росте при зміні формул — щоб звіти можна було зіставляти.
pub fn dict_version() -> String {
    wr_store::snapshot().text("DICT_VERSION").unwrap_or_else(|e| panic!("{e}")).to_string()
}

from_db!(
    /// Адреса з nonce > цього = біржа/сервіс (не приватний оператор).
    exchange_tx, "EXCHANGE_TX", u64
);
from_db!(
    /// Перцентиль degree для виключення хабів у графі (біржі/роутери).
    hub_pct, "HUB_PCT", f64
);
from_db!(
    /// Мін. розмір синхро-кластера.
    min_cluster, "MIN_CLUSTER", usize
);
from_db!(
    /// Вікно за замовчуванням, год.
    window_h_default, "WINDOW_H_DEFAULT", f64
);
from_db!(
    /// cluster/total >= це = викуп емісії.
    grab_emission, "GRAB_EMISSION", f64
);
from_db!(
    /// Мін. переказів для «засіву» у funding-BFS (одноразові відкидаємо).
    seed_min_transfers, "SEED_MIN_TRANSFERS", u32
);
from_db!(
    /// Аномальний native-баланс = біржа/пул (швидкий відсів у холдингах).
    whale_balance_anomaly_eth, "WHALE_BALANCE_ANOMALY_ETH", f64
);

// ── ЦІНИ (тимчасові; Фаза1.2 — реальні з prices.rs/tokprices) ───────────────
from_db!(price_eth_usd, "PRICE_ETH_USD", f64);
from_db!(price_matic_usd, "PRICE_MATIC_USD", f64);

// ── РЕАЛЬНІ ГРОШІ: контракти стейблів/WETH/WMATIC за МЕРЕЖАМИ ────────────────
/// Вид активу для P&L у реальних грошах. Тип живе у wr-store — один на всю систему.
pub use wr_store::Money;

/// Чи є контракт «справжніми грошима» (і скільки десяткових). Таблиця — у базі,
/// мережа поки 'eth' для зворотної сумісності зі старим однопараметровим викликом.
pub fn real_money(contract: &str) -> Option<(Money, u32)> {
    real_money_on("eth", contract)
}

/// Те саме, але з явною мережею: та сама таблиця покриває arbitrum і polygon.
pub fn real_money_on(chain: &str, contract: &str) -> Option<(Money, u32)> {
    wr_store::snapshot().real_money(chain, contract)
}

pub fn scale(raw: &str, dec: u32) -> f64 {
    raw.parse::<f64>().unwrap_or(0.0) / 10f64.powi(dec as i32)
}

/// USD-екв. суми реальних грошей.
pub fn to_usd(kind: Money, amount: f64) -> f64 {
    match kind {
        Money::Usd => amount,
        Money::Eth => amount * price_eth_usd(),
        Money::Matic => amount * price_matic_usd(),
    }
}

// ── СПАМ / СТЕЙБЛИ за СИМВОЛОМ (для токен-детекції, не для P&L) ──────────────
/// Символ-стейбл/базовий актив — НЕ сигнал координації.
pub fn is_base_symbol(sym: &str) -> bool {
    matches!(sym.to_lowercase().as_str(),
        "usdc"|"usdt"|"dai"|"weth"|"eth"|"wbtc"|"arb"|"usdce"|"frax"|"busd"|"tusd"|"usde"|"wmatic"|"matic")
}

/// Стейбл або загорнутий/LST — НЕ «альт»: природно churn-иться (мейкери й арбітраж),
/// тож у пошуку маніпуляцій альтами відсіюємо. Ловить USD/EUR/GBP-стейбли (вкл. aUSDC/cUSDC/USDP/…),
/// WBTC/cbBTC/tBTC, WETH/stETH/wstETH/rETH/cbETH/weETH та похідні.
pub fn is_stable_or_wrapped(sym: &str) -> bool {
    let s = sym.to_uppercase();
    s.contains("USD") || s.contains("EUR") || s.contains("GBP")
        || matches!(s.as_str(),
            "DAI"|"FRAX"|"LUSD"|"TUSD"|"GHO"|"MIM"|"USR"|"USTB"|"AUSD"|"PYUSD"|"USDE"|"USDL"|"USDX"
            |"WBTC"|"CBBTC"|"TBTC"|"BTCB"|"WETH"|"WBETH"|"CBETH"|"WSTETH"|"STETH"|"RETH"|"WEETH"|"EETH"|"SFRXETH"|"FRXETH"|"WMATIC"|"MATIC")
}

/// Спам-ейрдроп/скам: символ із пробілами/URL/claim/пунктуацією, або базовий актив.
pub fn is_spam_symbol(sym: &str) -> bool {
    let s = sym.trim();
    if s.is_empty() { return true }
    let clean = s.len() >= 2 && s.len() <= 10 && s.chars().all(|c| c.is_ascii_alphanumeric());
    if !clean { return true }
    is_base_symbol(s)
}

// ── ВІДОМІ БІРЖОВІ HOT-WALLET'и ─────────────────────────────────────────────
/// Мітка біржі за адресою. Таблиця переїхала у базу (`wallets`, kind='cex'),
/// куди зайшли і ці 7 адрес, і 46 рядків із rust-server/src/entities.rs —
/// раніше це були два незалежні переліки, які могли розійтись.
/// nonce-евристика (`exchange_tx`) лишається запасним шляхом.
pub fn known_exchange(addr: &str) -> Option<String> {
    let s = wr_store::snapshot();
    match s.wallet_kind("eth", addr) {
        Some("cex") => s.wallet_label("eth", addr).map(str::to_string),
        _ => None,
    }
}

// ── ФОРМАЛЬНІ ВИЗНАЧЕННЯ (L2 плану Oracul-Aladdin, Фаза 0.1) ──────────────────
// Одне поняття — одна формула — одне місце. Інструменти беруть визначення ЗВІДСИ.

// параметри визначень (єдині; змінюєш тут -> зростає DICT_VERSION)
from_db!(/// N: мін. операцій
    active_min_ops, "ACTIVE_MIN_OPS", u64);
from_db!(/// M: за останні M днів
    active_window_days, "ACTIVE_WINDOW_DAYS", u64);
from_db!(/// K%: частка ліквідності пулів, що вважається реалізовною
    cap_liq_fraction, "CAP_LIQ_FRACTION", f64);
from_db!(/// X: поріг кита за РЕАЛЬНИМ капіталом
    whale_min_usd, "WHALE_MIN_USD", f64);
from_db!(/// скільки з 3 ознак координації достатньо
    coord_min_signals, "COORD_MIN_SIGNALS", u8);

/// Гаманець активний = ≥ ACTIVE_MIN_OPS операцій за останні ACTIVE_WINDOW_DAYS днів.
pub fn is_active(ops_in_window: u64) -> bool { ops_in_window >= active_min_ops() }

/// Капітал групи (ПАПЕРОВИЙ) = сума позицій за спотовою ціною.
pub fn paper_capital(positions_usd: &[f64]) -> f64 { positions_usd.iter().sum() }

/// РЕАЛІЗОВНА вартість ОДНОГО активу = min(папір, frac·ліквідність пулів).
/// Це ПО-АКТИВНИЙ примітив (не можна продати актив A у ліквідність активу B).
/// frac — частка пулів, що вважається реалізовною (дефолт CAP_LIQ_FRACTION=0.10,
/// але capreal бере з env LIQ_FRAC — тож приймаємо параметром для байт-паритету).
pub fn realizable_usd(paper_usd: f64, pool_liq_usd: f64, frac: f64) -> f64 {
    paper_usd.min(frac * pool_liq_usd)
}

/// РЕАЛІЗОВНЕ з БІРЖОВОЮ ГЛИБИНОЮ (Фаза 1.2 плану L4): позицію можна вийти і в DEX-пули,
/// і в стакан біржі. cex_bid_depth = глибина bid ±2% (скільки купівельної ліквідності
/// абсорбує продаж без руху ціни >2%). realizable = min(папір, frac·DEX + біржовий bid).
/// ЗМІНЮЄ цифру відносно realizable_usd (це НОВА повнота, не байт-паритет).
pub fn realizable_with_cex(paper_usd: f64, pool_liq_usd: f64, frac: f64, cex_bid_depth_usd: f64) -> f64 {
    paper_usd.min(frac * pool_liq_usd + cex_bid_depth_usd)
}

/// Капітал групи (РЕАЛЬНИЙ) = нативний ETH(USD) + Σ реалізовне-по-активу.
/// ТОЧНА форма композиться з realizable_usd по кожному активу (як capreal):
///   real = native + Σ realizable_usd(paperᵢ, liqᵢ, K).
/// Ця функція — АГРЕГАТНЕ спрощення (один min на суму); для точності використовуй realizable_usd.
/// Саме тут $1.77млрд паперу → $377млн реальних (real-capital-liquidity).
pub fn real_capital(native_eth_usd: f64, paper_usd: f64, pool_liq_usd: f64) -> f64 {
    native_eth_usd + realizable_usd(paper_usd, pool_liq_usd, cap_liq_fraction())
}

/// Частка емісії = баланс групи / обіг токена (0..1). supply<=0 -> 0.
pub fn emission_share(group_balance: f64, token_supply: f64) -> f64 {
    if token_supply > 0.0 { group_balance / token_supply } else { 0.0 }
}

/// Частка емісії у ВІДСОТКАХ (готова для звіту/JSON). Порядок операцій `100*b/s`
/// НАВМИСНЕ — щоб збігатися БІТОВО зі старими бінами (holdercheck та ін.) при міграції.
/// Ратіо-форма — `emission_share`; через неасоціативність float `emission_share*100` може
/// відрізнятись від цього на ≤1 ULP, тож для % беремо цю канонічну форму репортингу.
pub fn emission_pct(balance: f64, supply: f64) -> f64 {
    if supply > 0.0 { 100.0 * balance / supply } else { 0.0 }
}

/// Роздача E = вийшло з групи / накопичено групою (0 = тримає, ≥1 = роздала все). accum<=0 -> 0.
pub fn distribution_e(out_of_group: f64, accumulated: f64) -> f64 {
    if accumulated > 0.0 { out_of_group / accumulated } else { 0.0 }
}

/// Кит = гаманець з РЕАЛЬНИМ капіталом ≥ WHALE_MIN_USD.
pub fn is_whale(real_capital_usd: f64) -> bool { real_capital_usd >= whale_min_usd() }

/// К-сть ознак координації (з 3): синхронний вхід, ідентичні портфелі, спільне джерело газу.
pub fn coordination_signals(synchronized: bool, identical_portfolio: bool, shared_gas: bool) -> u8 {
    synchronized as u8 + identical_portfolio as u8 + shared_gas as u8
}
/// Координація = ≥ COORD_MIN_SIGNALS ознак із трьох.
pub fn is_coordination(synchronized: bool, identical_portfolio: bool, shared_gas: bool) -> bool {
    coordination_signals(synchronized, identical_portfolio, shared_gas) >= coord_min_signals()
}

// ── РИЗИК-МОДЕЛЬ (Фаза 3 плану L6): ціновий вплив розпродажу ─────────────────
from_db!(/// Референсне ковзання, під яке міряна біржова глибина стаканів (±2%).
    slip_ref_pct, "SLIP_REF_PCT", f64);

/// Одностороння глибина DEX-пулу до SLIP_REF. Для constant-product (x·y=k) продаж,
/// що рухає ціну вниз на частку p, потребує ≈ reserve·(1/√(1−p) − 1). Для p=2% це
/// 1/√0.98 − 1 = 0.010153 → ~1.015% від резерву USD. Приводимо DEX-резерв до тих
/// самих одиниць, що й біржовий bid ±2%, аби додавати глибини коректно.
pub fn dex_depth_at_ref(reserve_usd: f64) -> f64 {
    reserve_usd * 0.010153
}

/// Лінійна оцінка ПРОСАДКИ (%) від продажу sell_usd при глибині depth_ref (USD до SLIP_REF):
///   impact = SLIP_REF · sell / depth_ref.
/// Це НИЖНЯ МЕЖА: реальний стакан тоншає глибше за ±2%, тож справжня просадка ≥ цієї.
/// depth_ref ≤ 0 → нема куди продати → INFINITY (позицію не вийти).
pub fn price_impact_pct(sell_usd: f64, depth_ref_usd: f64) -> f64 {
    if depth_ref_usd > 0.0 { slip_ref_pct() * sell_usd / depth_ref_usd } else { f64::INFINITY }
}

/// Метадані визначення для інспекції/аудиту: назва, формула, параметри, версія, дата зміни.
pub struct Definition {
    pub name: &'static str,
    pub formula: &'static str,
    pub params: &'static str,
    pub version: &'static str,
    pub since: &'static str,
}

/// РЕЄСТР визначень L2 — те, на що «посилаються всі» (Фаза 0.1).
pub fn definitions() -> &'static [Definition] {
    const V: &str = "1.0.0";
    const D: &str = "2026-08-09";
    &[
        Definition { name: "Гаманець активний", formula: "≥ N операцій за останні M днів",
            params: "N=ACTIVE_MIN_OPS(5), M=ACTIVE_WINDOW_DAYS(30)", version: V, since: D },
        Definition { name: "Група капіталу", formula: "зв'язна компонента за ребрами фінансування + спільним газом",
            params: "graph.rs union-find; HUB_PCT виключає хаби", version: V, since: D },
        Definition { name: "Капітал (паперовий)", formula: "Σ позицій × спот-ціна",
            params: "ціни з prices.rs (DefiLlama)", version: V, since: D },
        Definition { name: "Капітал (реальний)", formula: "native ETH + min(папір, K% ліквідності пулів)",
            params: "K=CAP_LIQ_FRACTION(0.10)", version: V, since: D },
        Definition { name: "Частка емісії", formula: "баланс групи / обіг токена",
            params: "—", version: V, since: D },
        Definition { name: "Роздача (E)", formula: "вийшло з групи / накопичено групою",
            params: "—", version: V, since: D },
        Definition { name: "Кит", formula: "реальний капітал ≥ $X",
            params: "X=WHALE_MIN_USD($1M)", version: V, since: D },
        Definition { name: "Координація", formula: "≥2 з 3: синхро-вхід + ідентичні портфелі + спільний газ",
            params: "COORD_MIN_SIGNALS(2)", version: V, since: D },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Once;

    /// Тести НЕ мають залежати від підключеного зовнішнього диска чи від того,
    /// що хтось уже виконав `wr-import load`. Тому власна тимчасова база
    /// з відомими значеннями — заразом це фіксує очікувані числа в самому тесті.
    fn fixture() {
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            let dir = std::env::temp_dir().join(format!("wr-dict-test-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let db = dir.join("t.db");
            let c = wr_store::open_or_create(&db).unwrap();
            for (k, v, kind) in [
                ("DICT_VERSION", "1.0.0 (2026-08-08)", "text"),
                ("EXCHANGE_TX", "50000", "int"),
                ("HUB_PCT", "0.98", "float"),
                ("MIN_CLUSTER", "8", "int"),
                ("WINDOW_H_DEFAULT", "24.0", "float"),
                ("GRAB_EMISSION", "0.9", "float"),
                ("SEED_MIN_TRANSFERS", "2", "int"),
                ("WHALE_BALANCE_ANOMALY_ETH", "10000.0", "float"),
                ("PRICE_ETH_USD", "3000.0", "float"),
                ("PRICE_MATIC_USD", "0.55", "float"),
                ("ACTIVE_MIN_OPS", "5", "int"),
                ("ACTIVE_WINDOW_DAYS", "30", "int"),
                ("CAP_LIQ_FRACTION", "0.10", "float"),
                ("WHALE_MIN_USD", "1000000.0", "float"),
                ("COORD_MIN_SIGNALS", "2", "int"),
                ("SLIP_REF_PCT", "2.0", "float"),
            ] {
                c.execute(
                    "INSERT INTO constants(key,value,kind,description) VALUES (?1,?2,?3,'test')",
                    (k, v, kind),
                )
                .unwrap();
            }
            c.execute("INSERT INTO chains VALUES ('eth','Ethereum','ETH',NULL,1)", []).unwrap();
            for (contract, sym, dec, mk) in [
                ("0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48", "USDC", 6, "usd"),
                ("0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2", "WETH", 18, "eth"),
            ] {
                c.execute(
                    "INSERT INTO tokens(chain,contract,symbol,decimals,money_kind,source)
                     VALUES ('eth',?1,?2,?3,?4,'test')",
                    (contract, sym, dec, mk),
                )
                .unwrap();
            }
            c.execute(
                "INSERT INTO wallets(chain,address,kind,label,source)
                 VALUES ('eth','0x28c6c06298d514db089934071355e5743bf21d60','cex','Binance 14','test')",
                [],
            )
            .unwrap();
            drop(c);
            // SAFETY: ставиться один раз, до першого звернення до знімка.
            unsafe { std::env::set_var("WR_DB", &db) };
        });
    }

    #[test]
    fn spam_filter() {
        assert!(is_spam_symbol("Claim rewards at https://x.io"));
        assert!(is_spam_symbol("ARB | t.me/claim"));
        assert!(is_spam_symbol("USDC"));
        assert!(is_spam_symbol(""));
        assert!(!is_spam_symbol("AIRENA"));
        assert!(!is_spam_symbol("GME"));
    }

    #[test]
    fn real_money_only_true_contracts() {
        fixture();
        assert!(matches!(
            real_money("0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48"),
            Some((Money::Usd, 6))
        ));
        assert!(matches!(
            real_money("0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2"),
            Some((Money::Eth, 18))
        ));
        assert!(real_money("0xdeadbeef").is_none());
        // регістр не має значення — нормалізується у сховищі
        assert!(real_money("0xA0b86991c6218B36c1d19D4a2e9Eb0cE3606eB48").is_some());
    }

    #[test]
    fn known_exchange_comes_from_the_store() {
        fixture();
        assert_eq!(
            known_exchange("0x28c6c06298d514db089934071355e5743bf21d60").as_deref(),
            Some("Binance 14")
        );
        assert!(known_exchange("0x0000000000000000000000000000000000000001").is_none());
    }

    #[test]
    fn thresholds_match_the_values_that_were_in_code() {
        fixture();
        // Числа мусять зійтись БІТОВО з колишніми літералами: інакше переїзд
        // тихо змінив би кожен поріг у системі.
        assert_eq!(hub_pct().to_bits(), (0.98f64).to_bits());
        assert_eq!(grab_emission().to_bits(), (0.9f64).to_bits());
        assert_eq!(cap_liq_fraction().to_bits(), (0.10f64).to_bits());
        assert_eq!(whale_min_usd().to_bits(), (1_000_000.0f64).to_bits());
        assert_eq!(price_eth_usd().to_bits(), (3000.0f64).to_bits());
        assert_eq!(price_matic_usd().to_bits(), (0.55f64).to_bits());
        assert_eq!(slip_ref_pct().to_bits(), (2.0f64).to_bits());
        assert_eq!(window_h_default().to_bits(), (24.0f64).to_bits());
        assert_eq!(whale_balance_anomaly_eth().to_bits(), (10_000.0f64).to_bits());
        assert_eq!(exchange_tx(), 50_000);
        assert_eq!(min_cluster(), 8);
        assert_eq!(seed_min_transfers(), 2);
        assert_eq!(coord_min_signals(), 2);
        assert_eq!(active_min_ops(), 5);
        assert_eq!(active_window_days(), 30);
        assert!(dict_version().starts_with("1.0.0"));
    }

    #[test]
    fn usd_conversion() {
        fixture();
        assert_eq!(to_usd(Money::Usd, 100.0), 100.0);
        assert_eq!(to_usd(Money::Eth, 2.0), 2.0 * price_eth_usd());
        assert_eq!(to_usd(Money::Matic, 1000.0), 1000.0 * price_matic_usd());
    }

    #[test]
    fn scale_decimals() {
        assert_eq!(scale("1000000", 6), 1.0);
        assert_eq!(scale("1000000000000000000", 18), 1.0);
    }

    #[test]
    fn definitions_l2() {
        fixture();
        assert!(is_active(5));
        assert!(!is_active(4));
        assert_eq!(real_capital(100.0, 900.0, 1000.0), 200.0);
        assert_eq!(real_capital(0.0, 50.0, 1000.0), 50.0);
        assert_eq!(
            realizable_usd(900.0, 1000.0, 0.10).to_bits(),
            (900.0f64).min(0.10 * 1000.0).to_bits()
        );
        assert_eq!(realizable_usd(50.0, 1000.0, 0.10), 50.0);
        assert_eq!(realizable_usd(1e9, 0.0, 0.10), 0.0);
        assert_eq!(realizable_with_cex(1e9, 1000.0, 0.10, 500.0), 600.0);
        assert_eq!(realizable_with_cex(400.0, 1000.0, 0.10, 500.0), 400.0);
        assert_eq!(realizable_with_cex(1e9, 0.0, 0.10, 0.0), 0.0);
        assert_eq!(
            realizable_with_cex(600.0, 1000.0, 0.10, 0.0).to_bits(),
            realizable_usd(600.0, 1000.0, 0.10).to_bits()
        );
        assert_eq!(price_impact_pct(1000.0, 1000.0), slip_ref_pct());
        assert_eq!(price_impact_pct(2000.0, 1000.0), 2.0 * slip_ref_pct());
        assert!(price_impact_pct(100.0, 0.0).is_infinite());
        assert!((dex_depth_at_ref(1_000_000.0) - 10_153.0).abs() < 1.0);
        assert_eq!(paper_capital(&[10.0, 20.0, 5.0]), 35.0);
        assert_eq!(emission_share(40.0, 100.0), 0.4);
        assert_eq!(emission_share(40.0, 0.0), 0.0);
        assert_eq!(emission_pct(40.0, 100.0).to_bits(), (100.0f64 * 40.0 / 100.0).to_bits());
        assert_eq!(emission_pct(5.0, 0.0), 0.0);
        assert_eq!(distribution_e(50.0, 100.0), 0.5);
        assert_eq!(distribution_e(10.0, 0.0), 0.0);
        assert!(is_whale(1_000_000.0));
        assert!(!is_whale(999_999.0));
        assert_eq!(coordination_signals(true, true, false), 2);
        assert!(is_coordination(true, false, true));
        assert!(!is_coordination(true, false, false));
        assert_eq!(definitions().len(), 8);
        assert!(definitions().iter().all(|d| !d.name.is_empty() && !d.formula.is_empty()));
    }
}
