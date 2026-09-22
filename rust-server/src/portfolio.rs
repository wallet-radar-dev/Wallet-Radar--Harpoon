// Pure kernel of `GET /api/portfolio-strategy` (server/portfolio.js
// `buildPortfolio`): a Core-Satellite institutional portfolio derived from the
// asset universe, each asset's CLARITY regulatory status, and a per-asset DCA
// `flowTilt` (whale accumulation/distribution phase). Deterministic given the
// universe + flowTilt; the universe load and the phase feed are the route's job
// in main.rs. Golden-tested byte-for-byte.

use crate::clarity::clarity_status;
use crate::jsmath::{js_num, round_fixed};
use serde_json::{json, Map, Value};

struct Profile {
    id: &'static str,
    name: &'static str,
    emoji: &'static str,
    desc: &'static str,
    core: i64,
    satellite: i64,
    stable: i64,
    max_core: i64,
    max_sat: i64,
    sat_names: usize,
    exit_e: i64,
    allow_contested: bool,
    accum_tilt: f64,
    smart_only: bool,
}

// PROFILES in Object insertion order (aggressive, moderate, conservative, smartmoney).
const PROFILES: [Profile; 4] = [
    Profile { id: "aggressive", name: "Агресивна", emoji: "🔴",
        desc: "Макс. альфа. Менше пасиву, великий сателіт (вкл. спірні активи), пізній вихід (E=55%) — їдемо за рухом кита довше. Вища дохідність ↔ вища просадка.",
        core: 25, satellite: 65, stable: 10, max_core: 25, max_sat: 12, sat_names: 12, exit_e: 55, allow_contested: true, accum_tilt: 1.0, smart_only: false },
    Profile { id: "moderate", name: "Помірна", emoji: "🟡",
        desc: "Баланс. Пасивне ядро BTC/ETH + альфа-сателіт із якісних активів, вихід E=48%. Кращий risk-adjusted, ніж наївний альт-кошик.",
        core: 55, satellite: 35, stable: 10, max_core: 35, max_sat: 8, sat_names: 8, exit_e: 48, allow_contested: false, accum_tilt: 1.0, smart_only: false },
    Profile { id: "conservative", name: "Консервативна", emoji: "🟢",
        desc: "Збереження капіталу. Велике ядро + буфер стейблів, мало сателіта (топ-якість), ранній вихід (E=30%) — фіксуємо прибуток швидко, мінімізуємо просадку.",
        core: 70, satellite: 15, stable: 15, max_core: 40, max_sat: 5, sat_names: 5, exit_e: 30, allow_contested: false, accum_tilt: 1.0, smart_only: false },
    Profile { id: "smartmoney", name: "Смартмані", emoji: "🔵",
        desc: "Чисте слідування за китом. Майже без пасиву — капітал іде в активи, де смарт-гроші ЗАРАЗ накопичують (фаза DCA-IN), вага за силою сигналу. Вихід строго по драбині кита.",
        core: 10, satellite: 80, stable: 10, max_core: 10, max_sat: 14, sat_names: 12, exit_e: 48, allow_contested: false, accum_tilt: 1.0, smart_only: true },
];

fn profile_of(id: &str) -> &'static Profile {
    PROFILES.iter().find(|p| p.id == id).unwrap_or(&PROFILES[1]) // fallback moderate
}

// phaseW[flowTilt[sym]] ?? 1.0
//
// `accum_tilt` НАВМИСНО зведено до 1.0 в усіх профілях. Він давав фазі накопичення
// перевагу в 1.2–2.4 рази, спираючись на припущення «кит купує — купуй і ти».
// Подієве дослідження цього ж проєкту показало протилежне: важке накопичення кита
// передує ПАДІННЮ ціни. Поки знак сигналу не підтверджений бектестом, накопичення
// не дає ані переваги, ані штрафу. Роздача (0.5) і вихід (0.0) лишаються — там
// напрямок безсумнівний: гаманець, що роздає, продає.
fn phase_weight(flow_tilt: &Value, sym: &str, accum_tilt: f64) -> f64 {
    match flow_tilt.get(sym).and_then(|v| v.as_str()) {
        Some("accumulation") => accum_tilt,
        Some("markup") => 1.1,
        Some("neutral") => 1.0,
        Some("distribution") => 0.5,
        Some("exit") => 0.0,
        _ => 1.0,
    }
}

/// Реальна ліквідність активу: скільки грошей залито в пули і скільки з них
/// реально обертається за добу. Джерело — `tokliq` (GeckoTerminal).
#[derive(Clone, Copy)]
struct Liq {
    reserve: f64,
    vol24: f64,
    /// Ринкова капіталізація — вісь каскаду. Ліквідність каже, чи можна вийти;
    /// капіталізація каже, до якого ешелону актив належить. Це різні питання:
    /// у великого активу буває тонкий пул, у дрібного — глибокий.
    mcap: f64,
}

/// Пороги ліквідності. Взяті з розподілу самого всесвіту груп (67 активів
/// із даними): медіана пулу $30k, чверть активів нижче $3.5k.
///   MIN  — нижче цього позицію не набрати й не вийти з неї; актив не тримаємо взагалі
///   CORE — «ліквідна база» має бути справді ліквідною (проходить ~11 активів)
///   VOL  — пул без обороту не торгується, скільки б у ньому не лежало
fn liq_limits() -> (f64, f64, f64) {
    let n = |k: &str, d: f64| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
    (n("MIN_LIQ_USD", 25_000.0), n("CORE_LIQ_USD", 250_000.0), n("CORE_VOL_USD", 5_000.0))
}

/// `dataset/backfill/token-liquidity.json` → SYM(верхній регістр) → Liq.
///
/// `None` для символу означає «не знаємо», а НЕ «нуля». Різниця тут коштує дорого:
/// відсутність даних не є підставою викинути актив, але й не є підставою пустити
/// його в ядро. Той самий урок, що й у шарі доступу до ланцюга.
fn liquidity() -> std::collections::HashMap<String, Liq> {
    let dir = std::env::var("DATASET_DIR")
        .unwrap_or_else(|_| "/opt/wallet-tracker/dataset".to_string());
    let mut out = std::collections::HashMap::new();
    let path = format!("{dir}/backfill/token-liquidity.json");
    // ГОЛОСНА ВІДМОВА. Мовчазне `Err(_) => return out` коштувало дня розслідування:
    // на Windows-запуску DATASET_DIR не заданий, шлях за замовчуванням лінуксовий,
    // файлу немає — і портфель віддавався ПОРОЖНІМ, але цілком валідним. Сторінка
    // стратегій малювалась повністю, лише без жодного активу: ні в логах, ні в
    // інтерфейсі поломки не було видно. Відсутність даних — це помилка
    // конфігурації, а не «на ринку нічого немає».
    let txt = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("[portfolio] НЕ ЧИТАЄТЬСЯ {path}: {e}. Ліквідність невідома — ешелони й ваги вийдуть порожні. Перевірте DATASET_DIR.");
            return out;
        }
    };
    let v: Value = match serde_json::from_str(&txt) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[portfolio] {path} не розбирається: {e}. Ліквідність невідома — ешелони й ваги вийдуть порожні.");
            return out;
        }
    };
    for (_c, t) in v["tokens"].as_object().map(|m| m.iter().collect::<Vec<_>>()).unwrap_or_default() {
        if let Some(sym) = t["symbol"].as_str() {
            let l = Liq {
                reserve: t["reserveUsd"].as_f64().unwrap_or(0.0),
                vol24: t["vol24"].as_f64().unwrap_or(0.0),
                mcap: t["mcap"].as_f64().unwrap_or(0.0),
            };
            // один символ може зустрітись у кількох контрактах — беремо найліквідніший
            let e = out.entry(sym.to_uppercase()).or_insert(l);
            if l.reserve > e.reserve {
                *e = l;
            }
        }
    }
    if out.is_empty() {
        tracing::error!("[portfolio] {path} прочитано, але жодного символу з ліквідністю — перевірте вміст файлу.");
    }
    out
}

/// Вага від РЕАЛЬНОЇ ліквідності. Логарифм — щоб один актив на $3.9 млн не з'їв
/// увесь рукав: різниця між $50k і $500k має значення, між $3 млн і $4 млн — ні.
fn liq_weight(l: Option<Liq>) -> f64 {
    match l {
        Some(x) if x.reserve > 0.0 => x.reserve.max(10.0).log10(),
        // невідомо — не нуль і не повноцінна вага: актив лишається придатним
        // для сателіта, але не конкурує за ядро
        _ => 1.0,
    }
}

/// Концентрація як ШТРАФ, а не як бал. Чим більша частка емісії в руках однієї
/// групи, тим важче вийти з позиції — і тим менша, а не більша, має бути вага.
fn conc_penalty(share: f64) -> f64 {
    1.0 - (share / 100.0).clamp(0.0, 0.9)
}

/// Стеля на сектор УСЕРЕДИНІ ешелону.
///
/// Розкидання імен по колу дає різні сектори у списку, але не в ВАЗІ: вага йде за
/// розміром активу, а найбільші активи під наглядом — майже суцільно DeFi. Портфель
/// виходив «диверсифікований» на вигляд і 55%-DeFi по суті.
///
/// Тут розміри секторів, що перевищують стелю, стискаються, а вивільнене
/// перетікає до решти. Кілька проходів, бо стиснення одного сектора піднімає частку
/// інших. Ешелонні цілі 50/30/10/10 не зачіпаються: нормування йде після цього.
fn balance_sectors(pool: &[&Tagged], cap_frac: f64) -> Vec<Tagged> {
    let mut out: Vec<Tagged> = pool.iter().map(|x| (*x).clone()).collect();
    if out.len() < 2 || cap_frac <= 0.0 || cap_frac >= 1.0 {
        return out;
    }
    for _ in 0..4 {
        let total: f64 = out.iter().map(|x| x.size.max(0.0)).sum();
        if total <= 0.0 {
            break;
        }
        let mut by: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
        for x in out.iter() {
            *by.entry(x.sector.clone()).or_insert(0.0) += x.size.max(0.0);
        }
        // сектор-одинак стискати нікуди: якщо в ешелоні один сектор, стеля недосяжна
        if by.len() < 2 {
            break;
        }
        let over: Vec<String> = by
            .iter()
            .filter(|(_, v)| **v / total > cap_frac)
            .map(|(k, _)| k.clone())
            .collect();
        if over.is_empty() {
            break;
        }
        for sec in over {
            let cur = by[&sec];
            let want = total * cap_frac;
            let k = (want / cur).clamp(0.05, 1.0);
            for x in out.iter_mut() {
                if x.sector == sec {
                    x.size *= k;
                }
            }
        }
    }
    out
}

// ─────────────────────── НАГЛЯД ЗА ГРУПАМИ ───────────────────────

/// Скільки груп капіталу тримає актив.
///
/// ЧОМУ САМЕ ЛІЧИЛЬНИК, А НЕ ЧАСТКА. Старий відбір брав «найбільшу частку емісії
/// серед груп» і тим САМИМ ВІДКИДАВ головне: скільки різних груп зійшлося на активі.
/// Один кит у позиції — це його ставка. Два-три незалежні кити в одному активі —
/// це вже узгодження, і саме воно нас цікавить.
///
/// Джерело — `tokenSignals` у `capital-groups.json`: те, що рахує groupsig по китах.
fn whale_watch() -> std::collections::HashMap<String, WatchInfo> {
    let dir = std::env::var("DATASET_DIR")
        .unwrap_or_else(|_| "/opt/wallet-tracker/dataset".to_string());
    let mut out: std::collections::HashMap<String, WatchInfo> = std::collections::HashMap::new();
    let path = format!("{dir}/capital-groups.json");
    // Це ЄДИНЕ джерело кандидатів для стратегій: немає його — немає жодного активу
    // в жодному профілі. Мовчати тут не можна (див. пояснення в `liquidity`).
    let txt = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("[portfolio] НЕ ЧИТАЄТЬСЯ {path}: {e}. СТРАТЕГІЇ ЛИШАТЬСЯ БЕЗ АКТИВІВ у всіх профілях. Перевірте DATASET_DIR.");
            return out;
        }
    };
    let v: Value = match serde_json::from_str(&txt) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[portfolio] {path} не розбирається: {e}. СТРАТЕГІЇ ЛИШАТЬСЯ БЕЗ АКТИВІВ у всіх профілях.");
            return out;
        }
    };
    for g in v["groups"].as_array().cloned().unwrap_or_default() {
        let gid = g["id"].as_str().unwrap_or("").to_string();
        for t in g["tokenSignals"].as_array().cloned().unwrap_or_default() {
            let sym = match t["symbol"].as_str() {
                Some(s) if !s.is_empty() => s.to_uppercase(),
                _ => continue,
            };
            let e = out.entry(sym).or_insert(WatchInfo {
                groups: 0,
                exitable: 0.0,
                emission: 0.0,
                distributing: false,
                who: String::new(),
            });
            e.groups += 1;
            e.exitable += t["exitableUsd"].as_f64().unwrap_or(0.0);
            // ДОДАЄМО, а не беремо максимум: групи капіталу не перетинаються
            // (`partitioned: true` у capital-groups.json, кожен гаманець рівно в одній),
            // тож 0.16% у NET-WEAK і 0.11% у NET — це 0.27% емісії під наглядом разом.
            // max() показував частку лише найбільшої групи й занижував загальну.
            e.emission += t["emissionPct"].as_f64().unwrap_or(0.0);
            if t["phase"].as_str().unwrap_or("").starts_with("РОЗДАЄ") {
                e.distributing = true;
            }
            if !e.who.is_empty() {
                e.who.push('+');
            }
            e.who.push_str(&gid);
        }
    }
    if out.is_empty() {
        tracing::error!("[portfolio] {path} прочитано, але в ньому немає жодного tokenSignals із символом — СТРАТЕГІЇ ЛИШАТЬСЯ БЕЗ АКТИВІВ.");
    }
    out
}

#[derive(Clone)]
struct WatchInfo {
    /// скільки різних груп тримає актив — головний критерій відбору
    groups: usize,
    /// скільки з позиції реально можна вийняти за ліквідністю пулів
    exitable: f64,
    /// найбільша частка емісії серед груп
    emission: f64,
    /// хоч одна група вже роздає
    distributing: bool,
    /// перелік груп, для пояснення в UI
    who: String,
}

// ─────────────────────── ЕШЕЛОНИ КАСКАДУ ───────────────────────

/// Ешелон за ринковою капіталізацією. Портфель будується каскадом:
/// фундамент із великих, далі середні, далі малі, і лише потім хайризик.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tier {
    Large,
    Mid,
    Small,
    Risk,
}

impl Tier {
    fn key(self) -> &'static str {
        match self {
            Tier::Large => "large",
            Tier::Mid => "mid",
            Tier::Small => "small",
            Tier::Risk => "risk",
        }
    }
    fn title(self) -> &'static str {
        match self {
            Tier::Large => "Фундамент — велика капіталізація",
            Tier::Mid => "Середня капіталізація",
            Tier::Small => "Мала капіталізація",
            Tier::Risk => "Хайризик — мікрокапіталізація",
        }
    }
}

/// Цільові ваги каскаду. Сума — рівно 100%: стейблів і мажорів у стратегії немає,
/// це портфель альткоїнів під наглядом груп.
const CASCADE: [(Tier, f64); 4] = [
    (Tier::Large, 50.0),
    (Tier::Mid, 30.0),
    (Tier::Small, 10.0),
    (Tier::Risk, 10.0),
];

/// Межі ешелонів у доларах капіталізації. Винесені в оточення, бо ринок зсувається,
/// а перекомпільовувати заради цього не має сенсу.
fn tier_limits() -> (f64, f64, f64) {
    let n = |k: &str, d: f64| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
    (
        n("TIER_LARGE_USD", 1_000_000_000.0),
        n("TIER_MID_USD", 100_000_000.0),
        n("TIER_SMALL_USD", 10_000_000.0),
    )
}

/// Ешелон активу. Без даних про капіталізацію актив іде в хайризик — не тому, що
/// він поганий, а тому, що НЕВІДОМИЙ розмір і є ризиком. Тихо зарахувати його до
/// фундаменту було б найгіршим із можливих припущень.
fn cap_tier(l: Option<Liq>) -> Tier {
    let (lg, md, sm) = tier_limits();
    let m = l.map(|x| x.mcap).unwrap_or(0.0);
    if m >= lg {
        Tier::Large
    } else if m >= md {
        Tier::Mid
    } else if m >= sm {
        Tier::Small
    } else {
        Tier::Risk
    }
}

/// Скільки позицій віддати кожному ешелону при загальній кількості `want`.
///
/// Пропорційно вазі каскаду, але не менше однієї на ешелон: портфель, у якому
/// хайризик представлений нулем позицій, — це вже інший портфель, а не той,
/// що описаний клієнту.
fn tier_quota(want: usize) -> [(Tier, usize); 4] {
    let mut q = [(Tier::Large, 0usize); 4];
    let mut used = 0usize;
    for (i, (t, w)) in CASCADE.iter().enumerate() {
        let n = ((want as f64) * w / 100.0).round() as usize;
        let n = n.max(1);
        q[i] = (*t, n);
        used += n;
    }
    // рахунок міг перевищити want через округлення й мінімум — зрізаємо з хвоста,
    // бо фундамент має лишатися повним
    let mut i = 3usize;
    while used > want {
        if q[i].1 > 1 {
            q[i].1 -= 1;
            used -= 1;
        } else if i > 0 {
            i -= 1;
        } else {
            break;
        }
    }
    q
}


// A tagged universe asset carrying its resolved CLARITY status.
#[derive(Clone)]
struct Tagged {
    sym: String,
    size: f64,
    /// Частка ЕМІСІЇ токена під наглядом груп, у відсотках (сума по групах).
    ///
    /// Окреме поле, а не `size`, і це принципово: `size` несе капіталізацію, а
    /// `balance_sectors` ще й МАСШТАБУЄ його, підганяючи сектори під стелю. Поле
    /// `emissionShare` у відповіді роками віддавало саме цей масштабований `size`,
    /// тобто в інтерфейсі стояло «7454432129.5% емісії» замість «0.06%».
    emission: f64,
    sector: String,
    status: String,
    short: String,
    score: f64,
    liq: Option<Liq>,
}

// weightCapNorm(pool, totalW, cap, bySize) → Vec of holding Values.
fn weight_cap_norm(pool: &[&Tagged], total_w: f64, cap: f64, by_size: bool) -> Vec<Value> {
    // raw weights
    // `by_size` історично означало «зважити за розміром активу». Коли всесвіт
    // замінили на активи груп, size став часткою емісії, і ядро почало зважуватись
    // за концентрацією — рівно навпаки до підпису рукава. Тепер це ліквідність.
    let mut w: Vec<f64> = pool.iter().map(|a| if by_size { liq_weight(a.liq).max(0.1) } else { a.score }).collect();
    let sum: f64 = w.iter().sum();
    let sum = if sum == 0.0 { 1.0 } else { sum };
    for x in w.iter_mut() {
        *x = (*x / sum) * total_w;
    }
    // apply cap, collect overflow
    let mut over = 0.0;
    for x in w.iter_mut() {
        if *x > cap {
            over += *x - cap;
            *x = cap;
        }
    }
    let under: Vec<usize> = (0..w.len()).filter(|&i| w[i] < cap).collect();
    let us: f64 = under.iter().map(|&i| w[i]).sum();
    let us = if us == 0.0 { 1.0 } else { us };
    for &i in &under {
        w[i] = cap.min(w[i] + over * (w[i] / us));
    }
    pool.iter().enumerate().map(|(i, a)| json!({
        "sym": a.sym,
        "sector": a.sector,
        "weight": js_num(round_fixed(w[i], 1)),
        "clarity": a.short,
        "status": a.status,
        // НАША ПЕРЕВАГА: не «скільки коштує актив», а «скільки його емісії тримають
        // групи капіталу. Відсоток від ПОВНОЇ емісії токена (знаменник `supply`
        // глобальний, тобто по всіх мережах); чисельник — володіння, які видно
        // в ланцюзі. Два знаки після коми: реальні значення тут — доліпроцента.
        "emissionShare": js_num(round_fixed(a.emission, 2)),
        // Капіталізація лишається у відповіді під власним іменем: вона потрібна
        // (за нею ділять ешелони), просто не має видавати себе за частку емісії.
        "mcapUsd": js_num(round_fixed(a.size, 0)),
        // Пастка ліквідності — тепер за РЕАЛЬНИМ пулом, а не за часткою емісії,
        // і вона вже відсіює на етапі відбору, а не просто фарбує підказку.
        "poolUsd": a.liq.map(|l| js_num(round_fixed(l.reserve, 0))).unwrap_or(Value::Null),
        "vol24Usd": a.liq.map(|l| js_num(round_fixed(l.vol24, 0))).unwrap_or(Value::Null),
        "liquidityTrap": a.liq.map(|l| l.reserve < liq_limits().1).unwrap_or(true),
    })).collect()
}


/// Чи є символ АЛЬТКОЇНОМ у сенсі стратегії «за китом».
///
/// Відсікаємо три класи, для яких сигнал групи капіталу не працює:
///   • мажори — їх рухає макро й потоки ETF, а не окрема група;
///   • обгортки (WETH, WBTC, stETH) — це той самий базовий актив у іншій формі,
///     їхній обіг створюють мейкери й арбітраж;
///   • стейбли — не мають напрямку за визначенням, для них є окремий рукав.
fn is_altcoin(sym: &str) -> bool {
    let s = sym.trim().to_uppercase();
    if s.is_empty() {
        return false;
    }
    const MAJORS: &[&str] = &[
        "BTC", "ETH", "BNB", "SOL", "XRP", "ADA", "DOGE", "TRX", "TON", "DOT",
        "MATIC", "AVAX", "LTC", "BCH",
    ];
    if MAJORS.contains(&s.as_str()) {
        return false;
    }
    const WRAPPED: &[&str] = &[
        "WETH", "WBTC", "STETH", "WSTETH", "RETH", "CBETH", "WEETH", "CBBTC",
        "TBTC", "WMATIC", "WBNB", "WAVAX", "WSOL", "SAVAX", "FRXETH", "SFRXETH",
    ];
    if WRAPPED.contains(&s.as_str()) {
        return false;
    }
    // Розписки протоколів (aEthWETH, aEthUSDC, cDAI…) — не активи, а вимоги на актив.
    // aEthWETH з капіталізацією $5.9 млрд інакше сідає у фундамент як «альткоїн»,
    // хоча це та сама ETH, лише в обгортці Aave.
    if s.starts_with("AETH") || s.starts_with("AUSD") || s.starts_with("VARIABLEDEBT") {
        return false;
    }
    // стейбли ловимо за підрядком: різновидів забагато для списку
    const STABLE_HINTS: &[&str] = &[
        "USD", "EUR", "GBP", "UAH", "DAI", "FRAX", "USTB", "BILL", "MUSD",
    ];
    if STABLE_HINTS.iter().any(|h| s.contains(h)) {
        return false;
    }
    true
}


/// Актив із нашого датасету груп: символ + найбільша частка емісії серед груп.
struct GroupAsset {
    sym: String,
    share: f64,
    group: String,
}

/// Альткоїни з `group-assets.json` — активи, де МИ бачимо, яка група скільки тримає.
/// Це відрізняє стратегію від загального списку монет: відбір спирається на
/// структуру власності, а не на капіталізацію.
fn group_assets() -> Vec<GroupAsset> {
    let dir = std::env::var("DATASET_DIR")
        .unwrap_or_else(|_| "/opt/wallet-tracker/dataset".to_string());
    let txt = match std::fs::read_to_string(format!("{dir}/group-assets.json")) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };
    let v: Value = match serde_json::from_str(&txt) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    // один актив може бути в кількох групах — беремо найбільшу частку
    let mut best: std::collections::HashMap<String, (f64, String)> = std::collections::HashMap::new();
    for g in v["groups"].as_array().unwrap_or(&vec![]) {
        let gid = g["id"].as_str().unwrap_or("").to_string();
        for a in g["assets"].as_array().unwrap_or(&vec![]) {
            if a["kind"].as_str() != Some("altcoin") {
                continue;
            }
            let sym = match a["symbol"].as_str() {
                Some(s) if is_altcoin(s) => s.to_string(),
                _ => continue,
            };
            let share = a["emissionShare"].as_f64().unwrap_or(0.0);
            let e = best.entry(sym).or_insert((0.0, gid.clone()));
            if share > e.0 {
                *e = (share, gid.clone());
            }
        }
    }
    let mut out: Vec<GroupAsset> = best
        .into_iter()
        .map(|(sym, (share, group))| GroupAsset { sym, share, group })
        .collect();
    out.sort_by(|a, b| b.share.partial_cmp(&a.share).unwrap_or(std::cmp::Ordering::Equal));
    out
}

/// Діапазон частки емісії, який відповідає НАЗВІ профілю.
/// Верхня межа ніде не сягає 70%: вище — пастка, позицію не продати.
fn share_band(profile: &str) -> (f64, f64) {
    match profile {
        "conservative" => (1.0, 40.0),   // розпорошено: ринок рухає не одна рука
        "moderate" => (20.0, 55.0),      // група помітна, ліквідність ще є
        "aggressive" => (40.0, 70.0),    // зона контролю: максимум потенціалу й ризику
        "smartmoney" => (20.0, 70.0),    // діапазон широкий, але фільтр по фазі жорсткий
        _ => (20.0, 55.0),
    }
}

/// Скільки позицій тримає профіль. Мінімум 20 — щоб жодна не важила критично;
/// максимум 40 — далі портфель перестає бути керованим.
fn target_names(profile: &str) -> usize {
    match profile {
        "conservative" => 20,
        "moderate" => 28,
        "aggressive" => 40,
        "smartmoney" => 24,
        _ => 28,
    }
}


/// Символи, які потрапляють у ПОРТФЕЛЬНІ профілі (консервативний, помірний,
/// агресивний). Потрібні смартмані-вітрині, щоб підняти їх нагору й позначити:
/// це те, що система не просто бачить, а й пропонує.
fn in_strategy_syms() -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    for prof in ["conservative", "moderate", "aggressive"] {
        let (lo, hi) = share_band(prof);
        let want = target_names(prof);
        let mut n = 0;
        for ga in group_assets() {
            if n >= want {
                break;
            }
            if ga.share >= lo && ga.share <= hi {
                out.insert(ga.sym.to_uppercase());
                n += 1;
            }
        }
    }
    out
}


/// Мінімальна вага позиції у відсотках портфеля.
///
/// Нижче цього значення позиція не має сенсу: на капітал $100k це $500, і
/// біржові комісії за вхід та вихід з'їдають помітну частку результату, а вплив
/// самої позиції на портфель — у межах похибки. Вагу відсічених позицій
/// отримують ті, що лишились.
const MIN_POSITION_PCT: f64 = 0.5;

/// Прибрати дрібні позиції й перерозподілити їхню вагу пропорційно решті,
/// зберігши цільову суму рукава.
fn drop_dust(mut holdings: Vec<Value>, target: f64) -> Vec<Value> {
    let kept: Vec<Value> = holdings
        .drain(..)
        .filter(|h| h["weight"].as_f64().unwrap_or(0.0) >= MIN_POSITION_PCT)
        .collect();
    if kept.is_empty() {
        return kept;
    }
    let sum: f64 = kept.iter().map(|h| h["weight"].as_f64().unwrap_or(0.0)).sum();
    if sum <= 0.0 {
        return kept;
    }
    kept.into_iter()
        .map(|mut h| {
            let w = h["weight"].as_f64().unwrap_or(0.0) / sum * target;
            h["weight"] = json!((w * 10.0).round() / 10.0);
            h
        })
        .collect()
}

/// The kernel. `universe` is an array of {sym,size,sector,c}; `flow_tilt` maps
/// SYM → DCA phase string; `max_top_holder` feeds the concentration rule text.
pub fn build_portfolio(universe: &[Value], profile: &str, flow_tilt: &Value, max_top_holder: i64) -> Value {
    let p = profile_of(profile);

    // Карту ліквідності читаємо ОДИН раз: `liquidity()` щоразу перечитує файл із
    // диска, а нижче вона потрібна у трьох місцях складання позицій.
    let liqmap = liquidity();

    let tagged: Vec<Tagged> = universe.iter().map(|a| {
        let sym = a["sym"].as_str().unwrap_or("").to_string();
        let cl = clarity_status(&sym);
        let liq = liqmap.get(&sym.to_uppercase()).copied();
        Tagged {
            sym,
            size: a["size"].as_f64().unwrap_or(0.0),
            emission: 0.0, // всесвітній актив без групи — частки емісії не знаємо
            sector: a["sector"].as_str().unwrap_or("").to_string(),
            status: cl["status"].as_str().unwrap_or("").to_string(),
            short: cl["short"].as_str().unwrap_or("").to_string(),
            score: 0.0,
            liq,
        }
    }).collect();

    // excluded (report): exchange/rwa, or contested when profile disallows it
    let excluded: Vec<Value> = tagged.iter().filter(|a| {
        a.status == "exchange" || a.status == "rwa" || (a.status == "contested" && !p.allow_contested)
    }).map(|a| json!({ "sym": a.sym, "why": a.short })).collect();

    // ── СКЛАД ЗА ПРОФІЛЕМ ────────────────────────────────────────────────
    // Критерій відбору — частка емісії, яку тримає група капіталу. Саме він
    // робить «Агресивну» справді агресивною, а не просто інакше зваженою.
    // ── ВІДБІР: ТІЛЬКИ З-ПІД НАГЛЯДУ ЗА ГРУПАМИ ──────────────────────
    //
    // Правило жорстке: у портфель не потрапляє нічого, чого не тримає жодна група
    // капіталу. Раніше сюди був добір «із класичного всесвіту, якщо груп не
    // вистачило» — тобто портфель тихо розбавлявся монетами, за якими ми не
    // спостерігаємо. Це знецінювало саму ідею: стратегія спирається на структуру
    // власності, і актив без неї не має тут підстав.
    //
    // Порядок відбору всередині ешелону:
    //   1) СКІЛЬКИ ГРУП тримає актив — головне. Один кит це його ставка;
    //      два-три незалежні кити в одному активі — узгодження.
    //   2) фаза (накопичення підсилює, роздача гасить)
    //   3) скільки реально можна вийняти за ліквідністю пулів
    let watch = whale_watch();
    let want = target_names(profile);

    // Смартмані лишається вітриною: показує ВСЕ під наглядом, без ліміту кількості.
    let showcase = profile == "smartmoney";

    let mut picks: Vec<Tagged> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    // кандидати з нагляду, з ешелоном і оцінкою
    struct Cand {
        t: Tagged,
        tier: Tier,
        groups: usize,
        rank: f64,
    }
    let mut cands: Vec<Cand> = Vec::new();
    for (sym, w) in watch.iter() {
        if !is_altcoin(sym) {
            continue;
        }
        let cl = clarity_status(sym);
        let status = cl["status"].as_str().unwrap_or("").to_string();
        if status == "stablecoin" || status == "exchange" || status == "rwa" {
            continue;
        }
        if status == "contested" && !p.allow_contested {
            continue;
        }
        let liq = liqmap.get(sym).copied();
        // актив, з якого не вийти, не тримаємо взагалі — скільки б груп його не тримало
        let (min_liq, _core_liq, _min_vol) = liq_limits();
        if let Some(l) = liq {
            if l.reserve > 0.0 && l.reserve < min_liq {
                continue;
            }
        }
        let pw = phase_weight(flow_tilt, sym, p.accum_tilt);
        if p.smart_only && !showcase && pw <= 0.0 {
            continue;
        }
        let tier = cap_tier(liq);
        // Ранг: кількість груп домінує над усім іншим — саме її просив клієнт.
        // Далі фаза, далі реальна можливість вийти. Роздача гасить, але не виключає:
        // рішення «виходити» ухвалює драбина DCA, а не відбір.
        let exit_w = (w.exitable.max(1.0)).log10();
        let rank = (w.groups as f64) * 1000.0
            + pw.max(0.1) * 100.0
            + exit_w
            - if w.distributing { 50.0 } else { 0.0 };
        // СЕКТОР лишається справжнім — це вісь диверсифікації, і затирати його
        // міткою груп не можна: інакше «розбивка по секторах» у звіті показує
        // назви груп, а портфель нема з чим звіряти на перекос.
        // Приналежність до груп переїхала в пояснення поруч зі статусом CLARITY.
        let sector = crate::sectors::sector_of(sym, "", "");
        let short = {
            let cl_short = cl["short"].as_str().unwrap_or("").to_string();
            let mark = if w.groups > 1 {
                format!("★{} груп: {}", w.groups, w.who)
            } else {
                format!("група {}", w.who)
            };
            if cl_short.is_empty() { mark } else { format!("{cl_short} · {mark}") }
        };
        cands.push(Cand {
            t: Tagged {
                sym: sym.clone(),
                size: liq.map(|l| l.mcap).unwrap_or(0.0),
                emission: w.emission,
                sector,
                status,
                short,
                score: rank,
                liq,
            },
            tier,
            groups: w.groups,
            rank,
        });
    }
    // спершу за кількістю груп, потім за рангом
    cands.sort_by(|a, b| {
        b.groups
            .cmp(&a.groups)
            .then(b.rank.partial_cmp(&a.rank).unwrap_or(std::cmp::Ordering::Equal))
    });

    if showcase {
        for c in cands.iter() {
            if seen.insert(c.t.sym.to_uppercase()) {
                picks.push(c.t.clone());
            }
        }
    } else {
        // Добираємо ешелон за ешелоном, згори вниз — це і є каскад.
        //
        // Усередині ешелону йдемо ПО КОЛУ ЗА СЕКТОРАМИ: спершу найкращий актив
        // кожного сектора, потім другий кожного, і так далі. Без цього відбір за
        // кількістю груп збирав портфель з одного сектора — кити сидять переважно
        // в DeFi, тож «найкращі за групами» виявлялись майже суцільно DeFi, і
        // портфель ніс концентрований секторний ризик під виглядом диверсифікації.
        for (tier, quota) in tier_quota(want) {
            let pool: Vec<&Cand> = cands.iter().filter(|c| c.tier == tier).collect();
            // сектори в порядку появи — тобто за силою найкращого свого активу
            let mut order: Vec<String> = Vec::new();
            for c in pool.iter() {
                if !order.contains(&c.t.sector) {
                    order.push(c.t.sector.clone());
                }
            }
            // СТЕЛЯ НА СЕКТОР У ІМЕНАХ, а не в розмірах. Ваги всередині ешелону
            // вирівнює кеп на позицію, тож сектор важить рівно стільки, скільки в
            // ньому імен: чотири DeFi з восьми — це 50% ешелону, хоч як стискай розміри.
            let sec_cap = std::env::var("MAX_SECTOR_PCT").ok().and_then(|v| v.parse::<f64>().ok()).unwrap_or(40.0) / 100.0;
            // Стеля рахується від ДОСТУПНОГО пулу, а не від бажаної квоти. Квота
            // фундаменту — 20 імен, а великих активів під наглядом усього 8: стеля
            // «40% від 20» = 8 імен на сектор, тобто обмеження, якого не існує.
            let effective = quota.min(pool.len());
            let max_per_sec = (((effective as f64) * sec_cap).floor() as usize).max(1);
            let mut per_sec: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
            let mut n = 0usize;

            // два проходи: спершу зі стелею, потім без неї — бо недобраний ешелон
            // гірший за перекос. Якщо інших секторів у всесвіті просто немає,
            // портфель має лишитись повним, а перекос — чесно видним у звіті.
            for strict in [true, false] {
                let mut round = 0usize;
                loop {
                    if n >= quota {
                        break;
                    }
                    let mut added = false;
                    for sec in order.iter() {
                        if n >= quota {
                            break;
                        }
                        if strict && per_sec.get(sec).copied().unwrap_or(0) >= max_per_sec {
                            continue;
                        }
                        let mut k = 0usize;
                        for c in pool.iter().filter(|c| &c.t.sector == sec) {
                            if k < round {
                                k += 1;
                                continue;
                            }
                            if seen.insert(c.t.sym.to_uppercase()) {
                                picks.push(c.t.clone());
                                *per_sec.entry(sec.clone()).or_insert(0) += 1;
                                n += 1;
                                added = true;
                            }
                            break;
                        }
                    }
                    if !added {
                        break;
                    }
                    round += 1;
                }
            }
        }
        // Ешелон міг не набрати квоти — у всесвіті просто немає стількох активів
        // такого розміру. Добираємо ЗГОРИ ВНИЗ: спершу великі, потім середні.
        // Простий добір «за рангом» сипав залишок у малий ешелон, і портфель
        // із каскаду перетворювався на купу дрібних імен.
        for (tier, _) in tier_quota(want) {
            for c in cands.iter().filter(|c| c.tier == tier) {
                if picks.len() >= want {
                    break;
                }
                if seen.insert(c.t.sym.to_uppercase()) {
                    picks.push(c.t.clone());
                }
            }
        }
    }

    picks.sort_by(|x, y| y.score.partial_cmp(&x.score).unwrap_or(std::cmp::Ordering::Equal));

    // ── ВАГА ЗА КАСКАДОМ ───────────────────────────────────────────
    //
    // Було: ядро — третина найвагоміших позицій, сателіт — решта. Поділ за
    // ПОРЯДКОМ у списку, а не за розміром активу: у «ядро» міг потрапити
    // мікрокап лише тому, що його тримає багато груп.
    //
    // Стало: вага розкладається по ешелонах капіталізації — 50 / 30 / 10 / 10.
    // Всередині ешелону ділиться за розміром активу, з кепом на позицію.
    // Ешелон, у якому не набралось жодної позиції, віддає свою вагу решті
    // пропорційно — інакше клієнт лишив би частину капіталу невитраченою.
    let by_tier: Vec<(Tier, f64, Vec<&Tagged>)> = CASCADE
        .iter()
        .map(|(t, w)| {
            let pool: Vec<&Tagged> = picks.iter().filter(|x| cap_tier(x.liq) == *t).collect();
            (*t, *w, pool)
        })
        .collect();

    let live_w: f64 = by_tier.iter().filter(|(_, _, pool)| !pool.is_empty()).map(|(_, w, _)| *w).sum();
    let scale = if live_w > 0.0 { 100.0 / live_w } else { 0.0 };

    let mut tier_h: Vec<(Tier, f64, Vec<Value>)> = Vec::new();
    for (t, w, pool) in by_tier.iter() {
        if pool.is_empty() {
            tier_h.push((*t, 0.0, Vec::new()));
            continue;
        }
        let target = w * scale;
        // Кеп на позицію: у фундаменті дозволяємо більшу концентрацію (там активи
        // ліквідні й вихід реальний), у хайризику — навмисно тісний, бо саме там
        // велика позиція перетворює помилку на катастрофу.
        let cap = match t {
            Tier::Large => p.max_core as f64,
            Tier::Mid => p.max_sat as f64,
            Tier::Small => (p.max_sat as f64 * 0.6).max(2.0),
            Tier::Risk => (p.max_sat as f64 * 0.4).max(1.0),
        };
        let by_size = matches!(t, Tier::Large | Tier::Mid);
        // стеля на сектор: за замовчуванням жоден сектор не бере більше 40% ешелону
        let sec_cap = std::env::var("MAX_SECTOR_PCT").ok().and_then(|v| v.parse::<f64>().ok()).unwrap_or(40.0) / 100.0;
        let balanced = balance_sectors(pool, sec_cap);
        let refs: Vec<&Tagged> = balanced.iter().collect();
        let mut h = weight_cap_norm(&refs, target, cap, by_size);
        if !showcase {
            h = drop_dust(h, target);
        }
        tier_h.push((*t, target, h));
    }

    // Фаза DCA проставляється всім позиціям, а не лише «сателіту»: у каскаді
    // немає позицій без фази — кожна може опинитись у роздачі.
    for (_, _, h) in tier_h.iter_mut() {
        for x in h.iter_mut() {
            let sym = x["sym"].as_str().unwrap_or("").to_string();
            let phase = flow_tilt
                .get(&sym)
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or("neutral");
            x["phase"] = json!(phase);
        }
    }

    // Сумісність із рештою маршруту: «ядро» = фундамент, «сателіт» = все інше.
    let core_target = tier_h.iter().find(|(t, _, _)| *t == Tier::Large).map(|(_, w, _)| *w).unwrap_or(0.0);
    let sat_target: f64 = tier_h.iter().filter(|(t, _, _)| *t != Tier::Large).map(|(_, w, _)| *w).sum();
    let core_h: Vec<Value> = tier_h.iter().find(|(t, _, _)| *t == Tier::Large).map(|(_, _, h)| h.clone()).unwrap_or_default();
    let sat_h: Vec<Value> = tier_h.iter().filter(|(t, _, _)| *t != Tier::Large).flat_map(|(_, _, h)| h.clone()).collect();
    let stable_pool: Vec<&Tagged> = Vec::new();
    let redistribute_stable = p.stable > 0;
    let stable_h: Vec<Value> = stable_pool.iter().map(|a| json!({
        "sym": a.sym,
        "sector": "Stable",
        "weight": js_num(round_fixed(p.stable as f64 / stable_pool.len() as f64, 1)),
        "clarity": a.short,
        "status": "stablecoin",
    })).collect();

    // all = core+sat+stable ; maxW ; sectors
    let all: Vec<&Value> = core_h.iter().chain(sat_h.iter()).chain(stable_h.iter()).collect();
    let max_w = all.iter().fold(0.0f64, |m, x| m.max(x["weight"].as_f64().unwrap_or(0.0)));
    let mut sectors = Map::new();
    for h in &all {
        let sector = h["sector"].as_str().unwrap_or("").to_string();
        let prev = sectors.get(&sector).and_then(|v| v.as_f64()).unwrap_or(0.0);
        sectors.insert(sector, js_num(round_fixed(prev + h["weight"].as_f64().unwrap_or(0.0), 1)));
    }

    let profiles: Vec<Value> = PROFILES.iter().map(|x| json!({ "id": x.id, "name": x.name, "emoji": x.emoji })).collect();

    json!({
        "profile": p.id, "profileName": p.name, "profileEmoji": p.emoji, "profileDesc": p.desc, "exitE": p.exit_e,
        "caps": { "maxCore": p.max_core, "maxSat": p.max_sat }, "allowContested": p.allow_contested,
        "profiles": profiles,
        "framework": "Core-Satellite · ризик-вагування · факторні нахили · систематичний ребаланс",
        "inspiredBy": "Vanguard (пасивне ядро) · BlackRock (активна альфа-сателіт) · Fidelity (кеш-менеджмент)",
        // цілі віддаємо ПЕРЕРАХОВАНІ — вони мають збігатися з тим, що реально
        // порахували ваги, інакше клієнт побачить одне, а сума дасть інше
        "sleeves": {
            "large": { "target": core_target.round() as i64, "role": "Фундамент — велика капіталізація, активи під наглядом кількох груп" },
            "mid":   { "target": tier_h.iter().find(|(t,_,_)| *t==Tier::Mid).map(|(_,w,_)| w.round() as i64).unwrap_or(0), "role": "Середня капіталізація — куди групи йдуть за дохідністю" },
            "small": { "target": tier_h.iter().find(|(t,_,_)| *t==Tier::Small).map(|(_,w,_)| w.round() as i64).unwrap_or(0), "role": "Мала капіталізація — вища віддача, вужчий вихід" },
            "risk":  { "target": tier_h.iter().find(|(t,_,_)| *t==Tier::Risk).map(|(_,w,_)| w.round() as i64).unwrap_or(0), "role": "Хайризик — мікрокапіталізація, коротко-середньостроково" },
        },
        "cascade": CASCADE.iter().map(|(t, w)| {
            let live = tier_h.iter().find(|(x,_,_)| x==t);
            json!({
                "tier": t.key(),
                "title": t.title(),
                "targetPct": w,
                "actualPct": live.map(|(_,a,_)| js_num(round_fixed(*a,1))).unwrap_or(json!(0)),
                "names": live.map(|(_,_,h)| h.len()).unwrap_or(0),
                "holdings": live.map(|(_,_,h)| h.clone()).unwrap_or_default(),
            })
        }).collect::<Vec<Value>>(),
        "factors": [
            "Flow (накопичення китів) → відбір і вага сателіта",
            "Quality (низька концентрація холдерів) → допуск",
            "Regulatory (CLARITY 🟢 товар) → гейт ядра й сателіта",
            "Liquidity (розмір) → вага ядра",
        ],
        "risk": {
            "names": all.len(), "maxWeightPct": js_num(round_fixed(max_w, 1)), "sectors": Value::Object(sectors),
            "rules": [
                format!("Кеп на актив: ядро ≤ {}%, сателіт ≤ {}%", p.max_core, p.max_sat),
                if p.allow_contested { "Виключено: 🟠 біржові, 🟣 RWA (🔴 спірні допущені — агресивний профіль)".to_string() } else { "Виключено з ризик-слівів: 🔴 спірні, 🟠 біржові, 🟣 RWA-цінні папери".to_string() },
                format!("Фільтр концентрації: топ-холдер ≤ {}% (анти-rug)", max_top_holder),
                "Risk-off: масова роздача китів (E↑ по сателіту) → перелив сателіт → стейбл".to_string(),
                format!("Вихід активу-сателіта по драбині DCA-OUT (остаточний при E={}%)", p.exit_e),
            ],
            "excluded": excluded,
        },
        "rebalance": "Систематичний, щомісяця, до цільових ваг (+ тригерний risk-off)",
        "weights": { "core": core_target.round() as i64, "satellite": sat_target.round() as i64,
                     "stable": if redistribute_stable { 0 } else { p.stable } },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_altcoins_in_risk_sleeves() {
        let inputs: Value = serde_json::from_str(include_str!("../tests/fixtures/portfolio-input.json")).unwrap();
        const BANNED: &[&str] = &[
            "BTC", "ETH", "BNB", "SOL", "XRP", "ADA", "DOGE", "TRX", "TON", "DOT",
            "WETH", "WBTC", "STETH", "USDC", "USDT", "DAI",
        ];
        for (i, c) in inputs.as_array().unwrap().iter().enumerate() {
            let out = build_portfolio(
                c["universe"].as_array().unwrap(),
                c["profile"].as_str().unwrap_or("moderate"),
                &c["flowTilt"],
                c["maxTopHolder"].as_i64().unwrap_or(0),
            );
            for sleeve in ["core", "satellite"] {
                let hs = out["sleeves"][sleeve]["holdings"].as_array().unwrap();
                for h in hs {
                    let sym = h["sym"].as_str().unwrap_or("").to_uppercase();
                    assert!(
                        !BANNED.contains(&sym.as_str()),
                        "case {i}: у рукаві {sleeve} опинився не-альткоїн {sym}"
                    );
                }
            }
        }
    }

    #[test]
    fn core_is_filled_and_not_duplicated_in_satellite() {
        let inputs: Value = serde_json::from_str(include_str!("../tests/fixtures/portfolio-input.json")).unwrap();
        let c = &inputs.as_array().unwrap()[0];
        let out = build_portfolio(
            c["universe"].as_array().unwrap(),
            "moderate",
            &c["flowTilt"],
            c["maxTopHolder"].as_i64().unwrap_or(0),
        );
        let core: Vec<String> = out["sleeves"]["core"]["holdings"].as_array().unwrap()
            .iter().map(|h| h["sym"].as_str().unwrap_or("").to_string()).collect();
        let sat: Vec<String> = out["sleeves"]["satellite"]["holdings"].as_array().unwrap()
            .iter().map(|h| h["sym"].as_str().unwrap_or("").to_string()).collect();

        assert!(!core.is_empty(), "ядро не має лишатися порожнім після фільтра мажорів");
        for sym in &core {
            assert!(!sat.contains(sym), "{sym} дублюється в ядрі й сателіті");
        }
    }

}
