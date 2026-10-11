#![allow(dead_code)]
//! tape_core — спільна основа стрічки угод: довідники бірж, свічки з розрізом агресора,
//! емісія активу, список спостереження.
//!
//! Навіщо винесено. Живий монітор (`selltape`) і ретро-реплей (`sigretro`) МУСЯТЬ рахувати
//! той самий сигнал тим самим кодом. Якби реплей мав власну копію порогів, емісії чи
//! списку активів, його результат не говорив би нічого про живий монітор: порівнювали б
//! дві різні програми. Тому все, що визначає, ЩО таке сигнал, живе тут в одному екземплярі,
//! а кожен бін додає лише своє — алерти в одного, прохід по історії в другого.
//!
//! Підключається як `#[path = "../tape_core.rs"] mod tape_core;` — у крейта `patterns`
//! немає бібліотечної цілі, і заводити її заради двох бінів надмірно.

use serde_json::Value;
use std::collections::HashMap;

pub const UA: &str = "wallet-radar-selltape/1.0";

// ───────────────────────────── дрібний інструмент ─────────────────────────────

pub fn env_s(k: &str, d: &str) -> String { std::env::var(k).unwrap_or_else(|_| d.to_string()) }
pub fn env_f(k: &str, d: f64) -> f64 { std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d) }
pub fn env_u(k: &str, d: u64) -> u64 { std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d) }

/// Скільки секунд у відрі: "5m" -> 300. Binance приймає ці ж рядки як interval.
pub fn tf_secs(tf: &str) -> i64 {
    let (num, unit) = tf.split_at(tf.len().saturating_sub(1));
    let n: i64 = num.parse().unwrap_or(5);
    match unit { "m" => n * 60, "h" => n * 3600, "d" => n * 86400, _ => 300 }
}

pub fn now_s() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Гроші коротко й читабельно: $1.24 млн замість 1240000.
pub fn usd(v: f64) -> String {
    let a = v.abs();
    let s = if a >= 1e9 { format!("${:.2} млрд", a / 1e9) }
        else if a >= 1e6 { format!("${:.2} млн", a / 1e6) }
        else if a >= 1e3 { format!("${:.0} тис.", a / 1e3) }
        else { format!("${a:.0}") };
    if v < 0.0 { format!("−{s}") } else { s }
}

pub fn price_fmt(p: f64) -> String {
    if p >= 100.0 { format!("${p:.2}") }
    else if p >= 1.0 { format!("${p:.4}") }
    else if p >= 0.0001 { format!("${p:.6}") }
    else { format!("${p:.10}") }
}

/// Година в UTC як «13:00» — час у повідомленні має бути однозначним.
pub fn hhmm(ts: i64) -> String {
    let d = ts.rem_euclid(86400);
    format!("{:02}:{:02}", d / 3600, (d % 3600) / 60)
}

pub fn median(v: &mut Vec<f64>) -> f64 {
    if v.is_empty() { return 0.0 }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = v.len();
    if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) / 2.0 }
}

pub fn get_json(url: &str) -> Option<Value> {
    match ureq::get(url).set("User-Agent", UA).timeout(std::time::Duration::from_secs(25)).call() {
        Ok(r) => r.into_json::<Value>().ok(),
        Err(e) => { eprintln!("[http] {url}: {e}"); None }
    }
}

pub fn f(v: &Value) -> f64 {
    v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())).unwrap_or(0.0)
}

pub fn read_json(path: &str) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

// ───────────────────────────── рядок стрічки ─────────────────────────────

#[derive(Clone)]
pub struct Row {
    pub ts: i64,            // початок відра, UTC, секунди
    pub symbol: String,     // тікер активу (UPPER)
    pub exchange: String,   // binance|okx|bybit|gate
    pub pair: String,
    pub close: f64,
    pub vol_usd: f64,
    pub buy_usd: f64,       // агресивні купівлі (taker buy)
    pub sell_usd: f64,      // агресивні продажі
    pub n: i64,
    pub src: String,        // klines|trades — чесно позначаємо, звідки розріз
}

impl Row {
    pub fn csv(&self) -> String {
        let bp = if self.vol_usd > 0.0 { 100.0 * self.buy_usd / self.vol_usd } else { 0.0 };
        format!("{},{},{},{},{:.10},{:.0},{:.0},{:.0},{},{:.1},{}",
            self.ts, self.symbol, self.exchange, self.pair, self.close,
            self.vol_usd, self.buy_usd, self.sell_usd, self.n, bp, self.src)
    }
}

// ───────────────────────────── довідники бірж ─────────────────────────────

/// Пари SYMBOL→pair на кожній біржі. Тримаємо добу в кеші: списки лістингів
/// міняються рідко, а тягнути 2 МБ exchangeInfo щогодини — марно.
pub struct Venues {
    pub binance: HashMap<String, String>,
    pub okx: HashMap<String, String>,
    pub bybit: HashMap<String, String>,
    pub gate: HashMap<String, String>,
}

pub fn cache_load(path: &str, max_age_s: i64) -> Option<Value> {
    let meta = std::fs::metadata(path).ok()?;
    let mt = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64;
    if now_s() - mt > max_age_s { return None }
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

pub fn load_venues(out: &str) -> Venues {
    let cache = format!("{out}/cache/venues.json");
    if let Some(v) = cache_load(&cache, 86400) {
        let m = |k: &str| -> HashMap<String, String> {
            v.get(k).and_then(|x| x.as_object()).map(|o|
                o.iter().map(|(a, b)| (a.clone(), b.as_str().unwrap_or("").to_string())).collect()
            ).unwrap_or_default()
        };
        let ven = Venues { binance: m("binance"), okx: m("okx"), bybit: m("bybit"), gate: m("gate") };
        if !ven.binance.is_empty() {
            eprintln!("[venues] з кешу: binance {} okx {} bybit {} gate {}",
                ven.binance.len(), ven.okx.len(), ven.bybit.len(), ven.gate.len());
            return ven
        }
    }

    let mut binance = HashMap::new();
    for host in ["https://api.binance.com", "https://data-api.binance.vision"] {
        let Some(v) = get_json(&format!("{host}/api/v3/exchangeInfo")) else { continue };
        if let Some(arr) = v.get("symbols").and_then(|x| x.as_array()) {
            for s in arr {
                if s.get("status").and_then(|x| x.as_str()) != Some("TRADING") { continue }
                if s.get("quoteAsset").and_then(|x| x.as_str()) != Some("USDT") { continue }
                let base = s.get("baseAsset").and_then(|x| x.as_str()).unwrap_or("").to_uppercase();
                let pair = s.get("symbol").and_then(|x| x.as_str()).unwrap_or("").to_string();
                if !base.is_empty() { binance.insert(base, pair); }
            }
        }
        if !binance.is_empty() { break }
    }

    let mut okx = HashMap::new();
    if let Some(v) = get_json("https://www.okx.com/api/v5/public/instruments?instType=SPOT") {
        if let Some(arr) = v.get("data").and_then(|x| x.as_array()) {
            for s in arr {
                if s.get("quoteCcy").and_then(|x| x.as_str()) != Some("USDT") { continue }
                if s.get("state").and_then(|x| x.as_str()) != Some("live") { continue }
                let base = s.get("baseCcy").and_then(|x| x.as_str()).unwrap_or("").to_uppercase();
                let id = s.get("instId").and_then(|x| x.as_str()).unwrap_or("").to_string();
                if !base.is_empty() { okx.insert(base, id); }
            }
        }
    }

    let mut bybit = HashMap::new();
    if let Some(v) = get_json("https://api.bybit.com/v5/market/instruments-info?category=spot&limit=1000") {
        if let Some(arr) = v.pointer("/result/list").and_then(|x| x.as_array()) {
            for s in arr {
                if s.get("quoteCoin").and_then(|x| x.as_str()) != Some("USDT") { continue }
                let base = s.get("baseCoin").and_then(|x| x.as_str()).unwrap_or("").to_uppercase();
                let id = s.get("symbol").and_then(|x| x.as_str()).unwrap_or("").to_string();
                if !base.is_empty() { bybit.insert(base, id); }
            }
        }
    }

    let mut gate = HashMap::new();
    if let Some(v) = get_json("https://api.gateio.ws/api/v4/spot/currency_pairs") {
        if let Some(arr) = v.as_array() {
            for s in arr {
                if s.get("quote").and_then(|x| x.as_str()) != Some("USDT") { continue }
                if s.get("trade_status").and_then(|x| x.as_str()) != Some("tradable") { continue }
                let base = s.get("base").and_then(|x| x.as_str()).unwrap_or("").to_uppercase();
                let id = s.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string();
                if !base.is_empty() { gate.insert(base, id); }
            }
        }
    }

    eprintln!("[venues] свіжо: binance {} okx {} bybit {} gate {}", binance.len(), okx.len(), bybit.len(), gate.len());
    let _ = std::fs::create_dir_all(format!("{out}/cache"));
    let dump = serde_json::json!({ "binance": binance, "okx": okx, "bybit": bybit, "gate": gate });
    let _ = std::fs::write(&cache, serde_json::to_string(&dump).unwrap_or_default());
    Venues { binance, okx, bybit, gate }
}

// ───────────────────────────── збір по одному активу ─────────────────────────────

/// Розбір відповіді klines. Поле 7 — оборот у котирувальній валюті (USDT),
/// поле 10 — з нього та частина, де агресором був покупець. Різниця цих двох
/// і є «продано агресором»; саме на ній тримається весь сигнал.
fn parse_klines(sym: &str, pair: &str, arr: &[Value]) -> Vec<Row> {
    let mut out = Vec::new();
    for k in arr {
        let Some(a) = k.as_array() else { continue };
        if a.len() < 11 { continue }
        let vol = f(&a[7]);
        let buy = f(&a[10]);
        out.push(Row {
            ts: a[0].as_i64().unwrap_or(0) / 1000,
            symbol: sym.to_string(), exchange: "binance".into(), pair: pair.to_string(),
            close: f(&a[4]), vol_usd: vol, buy_usd: buy, sell_usd: (vol - buy).max(0.0),
            n: a[8].as_i64().unwrap_or(0), src: "klines".into(),
        });
    }
    out
}

/// Binance: останні `limit` відер із чесним розрізом агресора.
pub fn from_binance(sym: &str, pair: &str, tf: &str, limit: u32) -> Vec<Row> {
    let mut v = None;
    for host in ["https://api.binance.com", "https://api1.binance.com", "https://data-api.binance.vision"] {
        v = get_json(&format!("{host}/api/v3/klines?symbol={pair}&interval={tf}&limit={limit}"));
        if v.is_some() { break }
    }
    let Some(arr) = v.as_ref().and_then(|x| x.as_array()) else { return Vec::new() };
    parse_klines(sym, pair, arr)
}

/// Те саме, але за ПЕРІОД у минулому, посторінково (біржа віддає максимум 1000 відер
/// за запит). Порожня відповідь означає кінець наявної історії, а не помилку: токен
/// міг бути залістингований пізніше за початок вікна.
///
/// Повертає `None`, якщо ЖОДНА сторінка не прийшла через мережу — це не те саме, що
/// «історії немає», і викликач мусить розрізняти ці випадки, а не ковтати тишу.
pub fn binance_range(sym: &str, pair: &str, tf: &str, from_s: i64, to_s: i64, sleep_ms: u64) -> Option<Vec<Row>> {
    let step = tf_secs(tf);
    if step <= 0 { return None }
    let mut out: Vec<Row> = Vec::new();
    let mut cursor = from_s;
    let mut any_response = false;
    while cursor < to_s {
        let url = format!(
            "https://api.binance.com/api/v3/klines?symbol={pair}&interval={tf}&startTime={}&endTime={}&limit=1000",
            cursor * 1000, to_s * 1000);
        let Some(v) = get_json(&url) else {
            // мережа/ліміт: чекаємо довше й пробуємо ще раз один раз
            std::thread::sleep(std::time::Duration::from_millis(2000));
            let Some(v2) = get_json(&url) else { break };
            any_response = true;
            let arr = v2.as_array().cloned().unwrap_or_default();
            if arr.is_empty() { break }
            let rows = parse_klines(sym, pair, &arr);
            cursor = rows.last().map(|r| r.ts + step).unwrap_or(to_s);
            out.extend(rows);
            std::thread::sleep(std::time::Duration::from_millis(sleep_ms));
            continue
        };
        any_response = true;
        let arr = v.as_array().cloned().unwrap_or_default();
        if arr.is_empty() { break }
        let rows = parse_klines(sym, pair, &arr);
        let last = rows.last().map(|r| r.ts).unwrap_or(cursor);
        out.extend(rows);
        if last + step <= cursor { break }   // біржа не рухається вперед — не зациклюємось
        cursor = last + step;
        std::thread::sleep(std::time::Duration::from_millis(sleep_ms));
    }
    if !any_response { return None }
    Some(out)
}

// ───────────────────────────── список спостереження й емісія ─────────────────────────────

/// Актив під наглядом: скільки емісії тримає група. Саме це перетворює
/// «просто злив на біржі» на «зливає той, за ким ми стежимо».
#[derive(Clone, Default)]
pub struct Watch { pub share: f64, pub group: String, pub contract: String }

/// Емісія активу в монетах. Ключ — контракт (тікери повторюються: SPA, FUN, REP
/// на різних контрактах — різні токени), з відкатом на тікер, коли контракту немає.
#[derive(Default)]
pub struct Supply { pub by_contract: HashMap<String, f64>, pub by_symbol: HashMap<String, f64> }

impl Supply {
    /// Емісія для активу з перевіркою на здоровий глузд.
    ///
    /// ЧОМУ НЕ ПРОСТО «взяти контракт групи». У даних груп ARB стоїть контрактом
    /// 0xb50721bc… з випуском 182 млн — це представлення токена на Ethereum, а торгується
    /// на біржах ARB із випуском 10 млрд. Порахувавши від меншого, ми отримали «40% емісії
    /// за добу» і ARB на першому місці звіту. Тому: беремо НАЙМЕНШУ з відомих емісій, у яку
    /// спостережений оборот ще вміщується (оборот ≤ 1/K емісії); якщо не вміщується в жодну —
    /// беремо найбільшу і кажемо прямо, що цифра про випуск сумнівна.
    pub fn units(&self, w: &Watch, sym: &str, flow_usd: f64, price: f64) -> (Option<f64>, &'static str) {
        // ETH не ERC-20 і в token-supply.json його нема; це та сама величина,
        // якою користується ринок, і вона потрібна, бо ETH — головний актив мережі
        if sym == "ETH" { return (Some(120_700_000.0), "") }

        let mut cands: Vec<f64> = Vec::new();
        if !w.contract.is_empty() {
            if let Some(v) = self.by_contract.get(&w.contract.to_lowercase()) { cands.push(*v) }
        }
        if let Some(v) = self.by_symbol.get(sym) { cands.push(*v) }
        cands.retain(|v| *v > 0.0);
        if cands.is_empty() || price <= 0.0 { return (None, "") }
        cands.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        // Стеля здорового глузду: у token-supply.json трапляються однойменні порожняки
        // з випуском 1e15 (наш «LINK» на $11 квадрильйонів). Жоден реальний актив не
        // коштує трильйонів, тож такі кандидати відкидаємо, а не показуємо 0.000%.
        const SANE_MAX_USD: f64 = 2e12;
        cands.retain(|c| c * price <= SANE_MAX_USD);
        if cands.is_empty() { return (None, "випуск у даних неправдоподібний — частки емісії немає") }

        const K: f64 = 4.0; // оборот більший за чверть усього випуску — це не випуск
        for c in &cands {
            if c * price >= flow_usd * K { return (Some(*c), "") }
        }
        (cands.last().copied(), "оборот не вміщується у відомий випуск — частка емісії приблизна")
    }
}

pub fn load_supply(ds: &str) -> Supply {
    let mut sp = Supply::default();
    let Some(v) = read_json(&format!("{ds}/token-supply.json")) else {
        eprintln!("[tape] token-supply.json не прочитано — частки емісії не буде");
        return sp
    };
    let Some(obj) = v.as_object() else { return sp };
    for (contract, t) in obj {
        let dec = t.get("decimals").and_then(|x| x.as_i64()).unwrap_or(18) as i32;
        // supply приходить рядком у найдрібніших одиницях: 1e27 в f64 не влізе точно,
        // але нам потрібні відсотки, а не вага до останнього wei
        let raw: f64 = t.get("supply").and_then(|x| x.as_str()).and_then(|x| x.parse().ok()).unwrap_or(0.0);
        if raw <= 0.0 { continue }
        let units = raw / 10f64.powi(dec);
        if !units.is_finite() || units <= 0.0 { continue }
        sp.by_contract.insert(contract.to_lowercase(), units);
        if let Some(sym) = t.get("symbol").and_then(|x| x.as_str()) {
            let k = sym.trim().to_uppercase();
            // на збігу тікерів беремо більшу емісію — вона майже завжди належить
            // тому токену, що реально торгується
            let e = sp.by_symbol.entry(k).or_insert(0.0);
            if units > *e { *e = units }
        }
    }
    sp
}

pub fn watchlist(ds: &str) -> HashMap<String, Watch> {
    let mut m: HashMap<String, Watch> = HashMap::new();
    let empty: Vec<Value> = vec![];
    {
        let mut put = |sym: &str, share: f64, group: &str, contract: &str| {
            let s = sym.trim().to_uppercase();
            // службові псевдотікери виду ETH:GUSDCQ на біржах не торгуються
            if s.is_empty() || s.len() > 12 || s.contains(':') { return }
            let e = m.entry(s).or_default();
            if e.contract.is_empty() && !contract.is_empty() { e.contract = contract.to_lowercase(); }
            if share > e.share { e.share = share; e.group = group.to_string(); }
        };

        // групи капіталу: частка емісії під контролем
        if let Some(v) = read_json(&format!("{ds}/group-assets.json")) {
            if let Some(gs) = v.get("groups").and_then(|x| x.as_array()) {
                for g in gs {
                    let id = g.get("id").and_then(|x| x.as_str()).unwrap_or("");
                    if id == "INFRA" || id == "NODATA" { continue } // не капітал групи
                    for a in g.get("assets").and_then(|x| x.as_array()).unwrap_or(&empty) {
                        if a.get("kind").and_then(|x| x.as_str()) == Some("stable") { continue }
                        if let Some(s) = a.get("symbol").and_then(|x| x.as_str()) {
                            put(s, a.get("emissionShare").and_then(|x| x.as_f64()).unwrap_or(0.0), id,
                                a.get("contract").and_then(|x| x.as_str()).unwrap_or(""));
                        }
                    }
                }
            }
        }
        // активи груп капіталу з застосунку
        if let Some(v) = read_json(&format!("{ds}/capital-group-assets.json")) {
            for a in v.get("assets").and_then(|x| x.as_array()).unwrap_or(&empty) {
                if a.get("real").and_then(|x| x.as_bool()) != Some(true) { continue }
                if let Some(s) = a.get("symbol").and_then(|x| x.as_str()) {
                    let g = a.get("capitalGroup").and_then(|x| x.as_str()).unwrap_or("група капіталу");
                    put(s, a.get("groupBought").and_then(|x| x.as_f64()).unwrap_or(0.0), g,
                        a.get("contract").and_then(|x| x.as_str()).unwrap_or(""));
                }
            }
        }
    }
    // Мажори, на яких сидить мережа — вони не завжди проходять поріг емісії, але саме
    // там реальні гроші. Контракт вказуємо явно: у token-supply.json на ті самі тікери
    // трапляються порожняки з абсурдним випуском, і без адреси «LINK» знаходився з
    // емісією 1e15 монет.

    // ETH не має контракту — рядок ("ETH", "") лишається тут, бо це символ,
    // а не адреса; решта пар приходить із бази за тегом.
    let mut majors: Vec<(String, String)> = vec![("ETH".to_string(), String::new())];
    majors.extend(
        wr_store::snapshot()
            .tokens_tagged("selltape:majors")
            .into_iter()
            .map(|t| (t.symbol_or_contract().to_string(), t.contract.clone())),
    );
    for (sym, contract) in &majors {
        let e = m.entry(sym.to_string()).or_insert(Watch { share: 0.0, group: "мажор мережі".into(), contract: String::new() });
        if e.contract.is_empty() { e.contract = contract.to_string() }
    }
    m
}
