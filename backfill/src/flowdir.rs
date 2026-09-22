//! flowdir — хто продає, а хто купує: напрямок кожного гаманця відносно бірж.
//!
//! Задача: знати, з яких гаманців емісія заходила НА біржі (намір продати) і які натомість
//! забирали З бірж (намір накопичити). Далі це дає причинність — що сталося після.
//!
//! ГОЛОВНА ПАСТКА, закладена в метод: **депозит на біржу не дорівнює продажу**. Ми вже
//! довели, що капітал у цій мережі ходить по колу (взаємні пари з симетрією 67–91%), а
//! середні точки входу й виходу майже збігаються (LINK $7.8596 → $7.8600). Адреса, яка
//! кладе на біржу і за кілька днів забирає назад, займається маркет-мейкінгом, а не
//! розвантаженням. Тому вирок виноситься за НЕТТО і за симетрією плечей, а не за окремими
//! переказами: якщо обидва плечі великі й близькі — це MAKER, і зараховувати його в
//! продавці означало б вигадати розпродаж там, де його немає.
//!
//! Друга пастка: «біржа» береться не з назви, а з міток і накопиченої бази (CEX-LABELED).
//! Адреса групи з біржовою міткою (їх у ядрі кілька) — це вихід НАЗОВНІ, і саме так
//! і рахується.
//!
//! Оцінка в USD — історичною ціною НА ДЕНЬ операції (token-prices-hist.json), бо продаж
//! 2021 року і продаж сьогодні за поточною ціною дали б однакове число, що безглуздо.
//!
//! Вихід: dataset/backfill/flowdir.json + патерни SELLER / BUYER / MAKER-CEX у базу.
//! Args: ROOT [файл-зі-списком]. Env: CHAIN(1) MIN_USD(10000) SYM_RATIO(0.7) TOP(30)
use backfill::known::{self, pat};
use backfill::patterns::{Finding, PatternDb};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, Write};

fn envn<T: std::str::FromStr>(k: &str, d: T) -> T {
    std::env::var(k).ok().and_then(|s| s.parse().ok()).unwrap_or(d)
}

/// Ціна на момент `ts` — найближча точка ряду. Ряди денні, тож найближча і є ціною дня.
fn price_at(series: &[(u64, f64)], ts: u64) -> Option<f64> {
    if series.is_empty() {
        return None;
    }
    let i = series.partition_point(|p| p.0 <= ts);
    let cand = match (i.checked_sub(1), series.get(i)) {
        (Some(a), Some(b)) => {
            if ts - series[a].0 <= b.0 - ts { series[a] } else { *b }
        }
        (Some(a), None) => series[a],
        (None, Some(b)) => *b,
        (None, None) => return None,
    };
    // точка за півроку від операції — це вже не ціна дня, краще чесно нічого не сказати
    if cand.0.abs_diff(ts) > 180 * 86400 {
        return None;
    }
    Some(cand.1)
}

/// Дата з мітки часу — щоб не тягнути календарну залежність у крейт.
fn ymd(ts: u64) -> String {
    if ts == 0 {
        return "—".into();
    }
    let days = ts / 86400;
    let (mut y, mut d) = (1970i64, days as i64);
    loop {
        let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
        let len = if leap { 366 } else { 365 };
        if d < len {
            break;
        }
        d -= len;
        y += 1;
    }
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let ml = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut m = 0;
    while m < 12 && d >= ml[m] {
        d -= ml[m];
        m += 1;
    }
    format!("{y}-{:02}-{:02}", m + 1, d + 1)
}

#[derive(Default, Clone)]
struct Leg {
    amt: f64,
    usd: f64,
    n: u32,
    t0: u64,
    t1: u64,
}

impl Leg {
    fn add(&mut self, amt: f64, usd: f64, ts: u64) {
        self.amt += amt;
        self.usd += usd;
        self.n += 1;
        if self.t0 == 0 || ts < self.t0 {
            self.t0 = ts
        }
        if ts > self.t1 {
            self.t1 = ts
        }
    }
}

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let bf = format!("{root}/dataset/backfill");
    let chain: u32 = envn("CHAIN", 1);
    let min_usd: f64 = envn("MIN_USD", 10_000.0);
    let sym_ratio: f64 = envn("SYM_RATIO", 0.7);
    let top: usize = envn("TOP", 30);
    let from_ts: u64 = envn("FROM_TS", 0);
    let to_ts: u64 = envn("TO_TS", u64::MAX);
    // Тег прогону: під ним патерн лягає в базу і під ним же зберігається JSON. Без тегу
    // прогін за 2024 рік затер би вирок за весь час, і порівняти їх стало б неможливо.
    let tag: String = std::env::var("TAG").unwrap_or_default();
    let sfx = if tag.is_empty() { String::new() } else { format!("-{tag}") };
    if !tag.is_empty() {
        eprintln!("[flowdir] прогін «{tag}» · вікно {from_ts}..{to_ts}");
    }

    let mut db = PatternDb::load(&bf);
    eprintln!("[flowdir] база патернів: {} адрес", db.map.len());

    // ── хто вважається біржею ─────────────────────────────────────────────────
    let mut cex: HashSet<String> = HashSet::new();
    let mut from_db_dep = 0usize;
    for (a, recs) in &db.map {
        if recs.iter().any(|r| r.pat == pat::CEX_LABELED) {
            cex.insert(a.clone());
        }
        // Депозитні адреси, знайдені попереднім прогоном, — теж біржа. Без цього кожен
        // наступний прогін мусив би заново шукати їх проходом по 124 млн рядків, а вони
        // вже лежать у базі: рівно те, заради чого база й накопичується.
        if recs.iter().any(|r| r.pat == "CEX-DEPOSIT-ADDR") {
            cex.insert(a.clone());
            from_db_dep += 1;
        }
    }
    if from_db_dep > 0 {
        eprintln!("[flowdir] депозитних адрес узято з бази: {from_db_dep}");
    }
    if let Ok(f) = File::open(format!("{bf}/labels-cache.json")) {
        if let Ok(m) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(f)) {
            for (a, v) in m {
                let mut s = v["name"].as_str().unwrap_or("").to_string();
                if let Some(arr) = v["labels"].as_array() {
                    for x in arr {
                        if let Some(t) = x.as_str() {
                            s.push(' ');
                            s.push_str(t)
                        }
                    }
                }
                if known::is_cex_label(&s) {
                    cex.insert(a.to_lowercase());
                }
            }
        }
    }
    // Фільтр за конкретною біржею: CEX_NAME=ftx рахує лише операції з адресами, чия мітка
    // містить це слово. Потрібне, щоб питати не «чи виходила мережа взагалі», а «чи
    // виходила вона саме звідти, і коли» — на загальних потоках така подія тоне.
    if let Ok(want) = std::env::var("CEX_NAME") {
        let w = want.to_lowercase();
        let mut only: HashSet<String> = HashSet::new();
        if let Ok(f) = File::open(format!("{bf}/labels-cache.json")) {
            if let Ok(m) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(f)) {
                for (a, v) in m {
                    let mut s = v["name"].as_str().unwrap_or("").to_string();
                    if let Some(arr) = v["labels"].as_array() {
                        for x in arr {
                            if let Some(t) = x.as_str() {
                                s.push(' ');
                                s.push_str(t)
                            }
                        }
                    }
                    let sl = s.to_lowercase();
                    // Пул DEX із назвою токена в мітці — не біржа. «SushiSwap: FTX Token»
                    // це ліквідність, а не рахунок на FTX.
                    if sl.contains("sushiswap")
                        || sl.contains("uniswap")
                        || sl.contains("balancer")
                        || sl.contains("curve")
                    {
                        continue;
                    }
                    // Збіг має бути за МЕЖЕЮ СЛОВА: інакше «nftx» проходить як «ftx»
                    // і тягне 16 сторонніх пулів проти 11 справжніх адрес біржі.
                    let ok = sl.match_indices(&w).any(|(i, _)| {
                        let before = i == 0 || !sl.as_bytes()[i - 1].is_ascii_alphanumeric();
                        let j = i + w.len();
                        let after = j >= sl.len() || !sl.as_bytes()[j].is_ascii_alphanumeric();
                        before && after
                    });
                    if ok {
                        only.insert(a.to_lowercase());
                    }
                }
            }
        }
        eprintln!("[flowdir] фільтр «{w}»: лишається {} адрес", only.len());
        cex = only;
    }
    eprintln!("[flowdir] біржових адрес: {}", cex.len());

    // Яка саме біржа стоїть за адресою. Потрібно, щоб не підключати ринкові дані всіх
    // майданчиків підряд, а лише тих, де наші адреси справді працюють: решта — марна
    // робота й зайві запити.
    let mut cex_name: HashMap<String, String> = HashMap::new();
    if let Ok(f) = File::open(format!("{bf}/labels-cache.json")) {
        if let Ok(m) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(f)) {
            for (a, v) in m {
                let mut s = v["name"].as_str().unwrap_or("").to_string();
                if let Some(arr) = v["labels"].as_array() {
                    for x in arr {
                        if let Some(t) = x.as_str() {
                            s.push(' ');
                            s.push_str(t)
                        }
                    }
                }
                let sl = s.to_lowercase();
                if let Some(w) = known::CEX_WORDS.iter().find(|w| {
                    // «deposit», «otc», «exchange» — це роль, а не назва майданчика
                    !matches!(**w, "deposit" | "otc" | "exchange") && sl.contains(*w)
                }) {
                    cex_name.insert(a.to_lowercase(), (*w).to_string());
                }
            }
        }
    }

    // ── члени, яких класифікуємо ──────────────────────────────────────────────
    let members: HashSet<String> = match std::env::args().nth(2) {
        Some(p) => BufReader::new(File::open(p).expect("список"))
            .lines()
            .map_while(Result::ok)
            .map(|l| l.trim().to_lowercase())
            .filter(|l| l.len() == 42)
            .collect(),
        None => db.map.keys().cloned().collect(),
    };
    eprintln!("[flowdir] членів до розбору: {}", members.len());

    // ── історичні ціни ────────────────────────────────────────────────────────
    let mut series: HashMap<String, Vec<(u64, f64)>> = HashMap::new();
    let mut symbol: HashMap<String, String> = HashMap::new();
    if let Ok(f) = File::open(format!("{bf}/token-prices-hist.json")) {
        if let Ok(hist) = serde_json::from_reader::<_, Value>(BufReader::new(f)) {
            // ряди лежать під "tokens", а не на верхньому рівні
            let root_v = if hist.get("tokens").is_some() { hist["tokens"].clone() } else { hist };
            if let Some(o) = root_v.as_object() {
                for (c, v) in o {
                    let mut pts: Vec<(u64, f64)> = v["prices"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default()
                        .iter()
                        .filter_map(|p| Some((p[0].as_u64()?, p[1].as_f64()?)))
                        .collect();
                    pts.sort_by_key(|p| p.0);
                    if pts.len() >= 10 {
                        series.insert(c.to_lowercase(), pts);
                    }
                    if let Some(s) = v["sym"].as_str() {
                        symbol.entry(c.to_lowercase()).or_insert_with(|| s.to_string());
                    }
                }
            }
        }
    }
    let eth_series = series.get("eth").cloned().unwrap_or_default();
    eprintln!("[flowdir] цінових рядів: {} (ETH: {} точок)", series.len(), eth_series.len());

    // ── ЕТАП 0: знайти ДЕПОЗИТНІ адреси бірж ──────────────────────────────────
    //
    // Без цього етапу картина систематично перекошена в бік купівель. Виведення з біржі
    // йде з гарячого гаманця, який має мітку, — його видно. А депозит клієнт шле на
    // ВЛАСНУ депозитну адресу біржі, і таких адрес мільйони, їх ніхто не мітить. Тому
    // перший прогін дав $33.6 млрд «купівель» проти $1.5 млрд «продажів» при портфелі
    // $5.74 млрд — тобто продажі просто не розпізнавались.
    //
    // Депозитну адресу видно з поведінки: вона приймає кошти й пересилає майже все на
    // мічену біржу, майже нічого не лишаючи собі. Саме цим вона й відрізняється від
    // звичайного гаманця, який теж інколи шле на біржу.
    let dep_ratio: f64 = envn("DEP_RATIO", 0.8);
    let dep_min: f64 = envn("DEP_MIN_USD", 1000.0);
    let mut deposits: HashSet<String> = HashSet::new();
    if std::env::var("SKIP_DEPDETECT").is_err() {
        // (адреса → скільки отримала всього, скільки віддала на мічену біржу) у токенах
        // не рахуємо — потрібне лише співвідношення, тож беремо сирі одиниці по кожному
        // активу окремо було б надто дорого. Рахуємо в ETH-еквіваленті подій: обсяг у
        // сирих одиницях порівнюється лише сам із собою в межах однієї адреси.
        let mut got: HashMap<String, f64> = HashMap::new();
        let mut to_ex: HashMap<String, f64> = HashMap::new();
        // депозитна адреса -> {біржа: обсяг}: куди саме вона зливає
        let mut dep_target: HashMap<String, HashMap<String, f64>> = HashMap::new();
        let f0 = File::open(format!("{bf}/txfull-{chain}.jsonl")).expect("txfull");
        let mut m = 0u64;
        for line in BufReader::with_capacity(1 << 22, f0).lines().map_while(Result::ok) {
            m += 1;
            if m % 40_000_000 == 0 {
                eprintln!("    [депозити] ...{m} рядків, кандидатів {}", got.len());
            }
            let Ok(t) = serde_json::from_str::<Value>(&line) else { continue };
            let kind = t["k"].as_str().unwrap_or("");
            if !known::is_eth_row(kind) && kind != "tok" {
                continue;
            }
            let from = t["f"].as_str().unwrap_or("").to_lowercase();
            let to = t["t"].as_str().unwrap_or("").to_lowercase();
            if from.is_empty() || to.is_empty() || from == to {
                continue;
            }
            let raw: f64 = t["v"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0.0);
            if raw <= 0.0 {
                continue;
            }
            let dec: i32 = if known::is_eth_row(kind) {
                18
            } else {
                t["dec"].as_str().and_then(|s| s.parse().ok()).unwrap_or(18)
            };
            let amt = raw / 10f64.powi(dec);

            // кандидатом може бути лише адреса, що НЕ біржа і НЕ наш член: депозитна
            // адреса завжди чужа й службова
            if !cex.contains(&to) && !members.contains(&to) {
                *got.entry(to.clone()).or_insert(0.0) += amt;
            }
            // Ціль збираємо НЕЗАЛЕЖНО від того, чи адреса вже відома як депозитна.
            // Інакше механізм самознищується: раз знайдені адреси потрапляють у набір
            // бірж і перестають бути кандидатами, тож повторний запуск дає нуль і назви
            // ніколи не з'являються.
            if let Some(nm) = cex_name.get(&to) {
                if !members.contains(&from) && !cex_name.contains_key(&from) {
                    *dep_target
                        .entry(from.clone())
                        .or_default()
                        .entry(nm.clone())
                        .or_insert(0.0) += amt;
                }
            }
            if cex.contains(&to) && !cex.contains(&from) && !members.contains(&from) {
                // Запам'ятовуємо не лише факт пересилання, а й КУДИ саме. Без цього
                // депозитна адреса лишається безіменною, і в сповіщенні монітора видно
                // «депозит на біржу» без назви майданчика — а саме назва й має значення:
                // з Binance і Coinbase ця мережа переважно виводить, а на crypto.com
                // і gate.io заводить.
                *to_ex.entry(from).or_insert(0.0) += amt;
            }
        }
        for (a, sent) in &to_ex {
            let recv = got.get(a).copied().unwrap_or(0.0);
            if recv >= dep_min && sent / recv.max(1e-9) >= dep_ratio {
                deposits.insert(a.clone());
            }
        }
        eprintln!(
            "[flowdir] депозитних адрес бірж знайдено: {} (з {} кандидатів)",
            deposits.len(),
            to_ex.len()
        );
        // Доповнюємо назвами і ті адреси, що вже лежать у базі з попередніх прогонів:
        // саме вони й лишились безіменними, через що монітор писав «депозит на біржу»
        // без майданчика.
        let known_dep: Vec<String> = db
            .map
            .iter()
            .filter(|(_, r)| r.iter().any(|x| x.pat == "CEX-DEPOSIT-ADDR"))
            .map(|(a, _)| a.clone())
            .collect();
        let mut all_dep: HashSet<String> = deposits.clone();
        all_dep.extend(known_dep);
        eprintln!("[flowdir] депозитних адрес усього (нові + з бази): {}", all_dep.len());

        let mut named = 0usize;
        for a in &all_dep {
            // майданчик визначаємо за найбільшим обсягом: депозитна адреса зазвичай
            // обслуговує одну біржу, але дрібні домішки трапляються
            let best = dep_target.get(a).and_then(|m| {
                m.iter().max_by(|x, y| x.1.partial_cmp(y.1).unwrap()).map(|(k, v)| (k.clone(), *v))
            });
            let mut f = Finding::new("CEX-DEPOSIT-ADDR", "flowdir").ev("ratio", dep_ratio);
            if let Some((nm, vol)) = best {
                f = f.evs("exchange", &nm).ev("volume", vol);
                named += 1;
            }
            db.add(a, f);
        }
        eprintln!("[flowdir] з них із визначеним майданчиком: {named}");
        // відтепер депозитна адреса рівносильна біржі
        for a in &deposits {
            cex.insert(a.clone());
        }
    }

    // ── один прохід ───────────────────────────────────────────────────────────
    // (гаманець → напрямок) і (гаманець, токен → напрямок)
    let mut to_cex: HashMap<String, Leg> = HashMap::new();
    let mut from_cex: HashMap<String, Leg> = HashMap::new();
    let mut per_tok: HashMap<(String, String), (Leg, Leg)> = HashMap::new();
    let mut tok_total: HashMap<String, (f64, f64)> = HashMap::new(); // sym -> (продано, куплено) USD
    // Денний ряд потоків. Річний підсумок ховає найцікавіше: під час кризи важливо не
    // скільки за рік, а чи встигли вийти ДО обвалу. Тому день -> (на біржі, з бірж).
    let daily_mode = std::env::var("DAILY").is_ok();
    // біржа -> (заведено, виведено, операцій)
    let mut by_exchange: HashMap<String, (f64, f64, u32)> = HashMap::new();
    // (біржа, рік) -> (заведено, виведено): чи стабільна роль майданчика в часі.
    // Разовий перекіс міг би бути випадком; повторення рік у рік — це вибір.
    let mut by_ex_year: HashMap<(String, i32), (f64, f64)> = HashMap::new();
    // біржа -> адреси, що з нею працюють: чи є спеціалізація учасників
    let mut ex_addrs: HashMap<String, HashSet<String>> = HashMap::new();
    let mut daily: HashMap<u64, (f64, f64)> = HashMap::new();
    // події для причинного аналізу: (контракт, час, USD, депозит?)
    let min_event: f64 = envn("MIN_EVENT_USD", 100_000.0);
    let mut events: Vec<(String, u64, f64, bool)> = Vec::new();

    let f = File::open(format!("{bf}/txfull-{chain}.jsonl")).expect("txfull");
    let mut n = 0u64;
    let mut cur_w = String::new();
    let mut seen: HashSet<u64> = HashSet::new();

    for line in BufReader::with_capacity(1 << 22, f).lines().map_while(Result::ok) {
        n += 1;
        if n % 20_000_000 == 0 {
            eprintln!("    ...{n} рядків");
        }
        let Ok(t) = serde_json::from_str::<Value>(&line) else { continue };
        let kind = t["k"].as_str().unwrap_or("");
        let from = t["f"].as_str().unwrap_or("").to_lowercase();
        let to = t["t"].as_str().unwrap_or("").to_lowercase();
        if from.is_empty() || to.is_empty() || from == to {
            continue;
        }
        // цікавить лише те, де один бік наш, а другий — біржа
        let (member, is_deposit) = if members.contains(&from) && cex.contains(&to) {
            (from.clone(), true)
        } else if members.contains(&to) && cex.contains(&from) {
            (to.clone(), false)
        } else {
            continue;
        };

        let w = t["w"].as_str().unwrap_or("");
        if w != cur_w {
            cur_w = w.to_string();
            seen.clear();
        }
        let mut h: u64 = 1469598103934665603;
        for s in [t["h"].as_str().unwrap_or(""), from.as_str(), to.as_str(), t["v"].as_str().unwrap_or("")] {
            for b in s.as_bytes() {
                h ^= *b as u64;
                h = h.wrapping_mul(1099511628211)
            }
        }
        if !seen.insert(h) {
            continue;
        }

        let ts = t["ts"].as_u64().unwrap_or(0);
        // Часове вікно. Роль гаманця не вічна: та сама адреса може набирати два роки,
        // а потім розвантажуватись. Один вирок «за весь час» це усереднює й ховає
        // момент розвороту, тому прогін роблять і по роках.
        if ts < from_ts || ts > to_ts {
            continue;
        }
        let raw: f64 = t["v"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0.0);
        if raw <= 0.0 {
            continue;
        }

        let (amt, usd, sym, contract) = if known::is_eth_row(kind) {
            let a = raw / 1e18;
            (a, price_at(&eth_series, ts).map(|p| a * p).unwrap_or(0.0), "ETH".to_string(), "eth".to_string())
        } else if kind == "tok" {
            let dec: i32 = t["dec"].as_str().and_then(|s| s.parse().ok()).unwrap_or(18);
            let a = raw / 10f64.powi(dec);
            let c = t["tok"].as_str().unwrap_or("").to_lowercase();
            let s = t["sym"].as_str().unwrap_or("").to_string();
            let u = series.get(&c).and_then(|sr| price_at(sr, ts)).map(|p| a * p).unwrap_or(0.0);
            (a, u, s, c)
        } else {
            continue;
        };
        // велика операція — кандидат у подію для причинного аналізу
        if usd >= min_event {
            events.push((contract, ts, usd, is_deposit));
        }
        if amt <= 0.0 {
            continue;
        }

        // облік по майданчиках: хто з них справді бачить наші кошти.
        // Якщо «наша» адреса сама є біржею, це рух між майданчиками, а не активність
        // групи — і саме так Bitfinex зі списку членів колись дав $8.4 млрд фікції.
        if !cex.contains(&member) {
            let other = if is_deposit { &to } else { &from };
            if let Some(nm) = cex_name.get(other) {
                let e = by_exchange.entry(nm.clone()).or_insert((0.0, 0.0, 0u32));
                if is_deposit {
                    e.0 += usd
                } else {
                    e.1 += usd
                }
                e.2 += 1;
                let year = 1970 + (ts as f64 / 31_556_952.0).floor() as i32;
                let y = by_ex_year.entry((nm.clone(), year)).or_insert((0.0, 0.0));
                if is_deposit { y.0 += usd } else { y.1 += usd }
                ex_addrs.entry(nm.clone()).or_default().insert(member.clone());
            }
        }
        if daily_mode {
            let d = daily.entry(ts / 86400).or_insert((0.0, 0.0));
            if is_deposit {
                d.0 += usd
            } else {
                d.1 += usd
            }
        }
        let e = per_tok.entry((member.clone(), sym.clone())).or_default();
        if is_deposit {
            to_cex.entry(member).or_default().add(amt, usd, ts);
            e.0.add(amt, usd, ts);
            tok_total.entry(sym).or_insert((0.0, 0.0)).0 += usd;
        } else {
            from_cex.entry(member).or_default().add(amt, usd, ts);
            e.1.add(amt, usd, ts);
            tok_total.entry(sym).or_insert((0.0, 0.0)).1 += usd;
        }
    }
    eprintln!("[flowdir] рядків {n}");

    // ── вирок по кожному гаманцю ──────────────────────────────────────────────
    let mut all: HashSet<&String> = HashSet::new();
    all.extend(to_cex.keys());
    all.extend(from_cex.keys());

    let mut rows: Vec<Value> = Vec::new();
    let (mut n_sell, mut n_buy, mut n_maker, mut n_small) = (0usize, 0usize, 0usize, 0usize);
    let (mut sell_usd, mut buy_usd) = (0f64, 0f64);

    let mut n_cex_self = 0usize;
    for a in &all {
        // Адреса, яка сама має біржову мітку, класифікації не підлягає: її «депозити» —
        // це рух біржі, а не рішення учасника. Без цього Bitfinex `0x742d35cc…`, що
        // потрапила в список членів, дала $8.4 млрд фіктивного «продажу» — половину
        // всього підсумку.
        if cex.contains(*a) {
            n_cex_self += 1;
            continue;
        }
        let d = to_cex.get(*a).cloned().unwrap_or_default();
        let w = from_cex.get(*a).cloned().unwrap_or_default();
        let gross = d.usd + w.usd;
        if gross < min_usd {
            n_small += 1;
            continue;
        }
        let net = d.usd - w.usd; // >0 — віддав на біржі, <0 — забрав з бірж
        // симетрія плечей: 1.0 означає, що скільки поклав, стільки й забрав
        let symv = if d.usd.max(w.usd) > 0.0 { d.usd.min(w.usd) / d.usd.max(w.usd) } else { 0.0 };

        let verdict = if symv >= sym_ratio {
            // обидва плечі великі й близькі — прокрутка, а не позиція
            n_maker += 1;
            "MAKER-CEX"
        } else if net > 0.0 {
            n_sell += 1;
            sell_usd += net;
            "SELLER"
        } else {
            n_buy += 1;
            buy_usd += -net;
            "BUYER"
        };

        // під тегом прогону: вирок «за весь час» і вирок за 2024-й — різні твердження
        let pat_name = if tag.is_empty() { verdict.to_string() } else { format!("{verdict}@{tag}") };
        db.add(
            a,
            Finding::new(&pat_name, "flowdir")
                .ev("netUsd", net)
                .ev("toCexUsd", d.usd)
                .ev("fromCexUsd", w.usd)
                .ev("symmetry", symv),
        );

        rows.push(json!({"addr": a, "verdict": verdict, "netUsd": net,
            "toCexUsd": d.usd, "fromCexUsd": w.usd, "symmetry": symv,
            "nDeposits": d.n, "nWithdrawals": w.n,
            "firstDep": d.t0, "lastDep": d.t1, "firstWd": w.t0, "lastWd": w.t1}));
    }

    rows.sort_by(|a, b| {
        b["netUsd"].as_f64().unwrap_or(0.0).abs().partial_cmp(&a["netUsd"].as_f64().unwrap_or(0.0).abs()).unwrap()
    });

    println!("\n=== ПІДСУМОК: ХТО ЩО РОБИТЬ ===");
    println!("ПРОДАВЦІ (нетто на біржі):   {n_sell:>5} адрес · ${sell_usd:.0}");
    println!("ПОКУПЦІ  (нетто з бірж):     {n_buy:>5} адрес · ${buy_usd:.0}");
    println!("ПРОКРУТКА (симетрія ≥{sym_ratio}):  {n_maker:>5} адрес — ні продаж, ні купівля");
    println!("надто дрібні (<${min_usd:.0}):  {n_small:>5} адрес");
    println!("самі біржі (не рахуємо):     {n_cex_self:>5} адрес");
    println!("\nЧИСТИЙ НАПРЯМОК МЕРЕЖІ: ${:.0}", sell_usd - buy_usd);

    println!("\n=== НАЙБІЛЬШІ ПРОДАВЦІ ===");
    println!("{:<44} {:>15} {:>13} {:>13} {:>5}", "АДРЕСА", "НЕТТО НА БІРЖІ $", "ПОКЛАВ $", "ЗАБРАВ $", "СИМ");
    for r in rows.iter().filter(|r| r["verdict"] == "SELLER").take(top) {
        println!(
            "{} {:>15.0} {:>13.0} {:>13.0} {:>5.2}",
            r["addr"].as_str().unwrap_or(""),
            r["netUsd"].as_f64().unwrap_or(0.0),
            r["toCexUsd"].as_f64().unwrap_or(0.0),
            r["fromCexUsd"].as_f64().unwrap_or(0.0),
            r["symmetry"].as_f64().unwrap_or(0.0)
        );
    }

    println!("\n=== НАЙБІЛЬШІ ПОКУПЦІ ===");
    for r in rows.iter().filter(|r| r["verdict"] == "BUYER").take(top) {
        println!(
            "{} {:>15.0} {:>13.0} {:>13.0} {:>5.2}",
            r["addr"].as_str().unwrap_or(""),
            -r["netUsd"].as_f64().unwrap_or(0.0),
            r["toCexUsd"].as_f64().unwrap_or(0.0),
            r["fromCexUsd"].as_f64().unwrap_or(0.0),
            r["symmetry"].as_f64().unwrap_or(0.0)
        );
    }

    // ── що саме розвантажують, а що набирають ─────────────────────────────────
    let mut toks: Vec<(&String, &(f64, f64))> = tok_total.iter().collect();
    toks.sort_by(|a, b| (b.1 .0 - b.1 .1).abs().partial_cmp(&(a.1 .0 - a.1 .1).abs()).unwrap());
    println!("\n=== ПО АКТИВАХ: ЩО ВІДДАЮТЬ, ЩО НАБИРАЮТЬ ===");
    println!("{:<12} {:>15} {:>15} {:>15}", "ТОКЕН", "НА БІРЖІ $", "З БІРЖ $", "НЕТТО $");
    for (s, (d, w)) in toks.iter().take(top) {
        if d + w < min_usd {
            continue;
        }
        println!("{:<12} {:>15.0} {:>15.0} {:>15.0}", s, d, w, d - w);
    }

    // ── ДЕ САМЕ ПРАЦЮЮТЬ НАШІ АДРЕСИ ──────────────────────────────────────────
    if !by_exchange.is_empty() {
        let mut ex: Vec<(&String, &(f64, f64, u32))> = by_exchange.iter().collect();
        ex.sort_by(|a, b| (b.1 .0 + b.1 .1).partial_cmp(&(a.1 .0 + a.1 .1)).unwrap());
        println!("
=== БІРЖІ, ДЕ Є АКТИВНІСТЬ НАШИХ АДРЕС ===");
        println!("{:<14} {:>15} {:>15} {:>15} {:>9}", "БІРЖА", "ЗАВЕДЕНО $", "ВИВЕДЕНО $", "ОБІГ $", "ОПЕРАЦІЙ");
        let tot: f64 = ex.iter().map(|x| x.1 .0 + x.1 .1).sum();
        for (n, (d, w, c)) in &ex {
            let turn = d + w;
            println!("{:<14} {:>15.0} {:>15.0} {:>15.0} {:>9}  {:.1}%", n, d, w, turn, c, 100.0 * turn / tot.max(1.0));
        }
        // ── чи стабільна роль майданчика ──────────────────────────────────────
        println!("
=== РОЛЬ МАЙДАНЧИКА ПО РОКАХ (нетто, + заводять / - виводять) ===");
        print!("{:<12}", "БІРЖА");
        for y in 2020..=2026 { print!(" {:>9}", y); }
        println!("  {:>7}", "АДРЕС");
        for (n, _) in ex.iter().take(9) {
            print!("{:<12}", n);
            for y in 2020..=2026 {
                match by_ex_year.get(&((*n).clone(), y)) {
                    Some((d, w)) if d + w > 1e6 => print!(" {:>+9.0}", (d - w) / 1e6),
                    _ => print!(" {:>9}", "·"),
                }
            }
            println!("  {:>7}", ex_addrs.get(*n).map(|s| s.len()).unwrap_or(0));
        }
        println!("(млн $; · = обіг менший за $1 млн)");

        let _ = File::create(format!("{bf}/exchanges{sfx}.json")).unwrap().write_all(
            serde_json::to_string(&json!({"exchanges": ex.iter().map(|(n, (d, w, c))|
                json!({"name": n, "toCexUsd": d, "fromCexUsd": w, "turnoverUsd": d + w, "ops": c}))
                .collect::<Vec<_>>()})).unwrap().as_bytes());
    }

    // ── ДЕННИЙ РЯД ────────────────────────────────────────────────────────────
    if daily_mode && !daily.is_empty() {
        let mut days: Vec<(&u64, &(f64, f64))> = daily.iter().collect();
        days.sort_by_key(|x| *x.0);
        println!("\n=== ДЕННІ ПОТОКИ ===");
        println!("{:<12} {:>15} {:>15} {:>15}", "ДАТА", "НА БІРЖІ $", "З БІРЖ $", "НЕТТО $");
        for (d, (to_c, from_c)) in &days {
            if to_c + from_c < 1000.0 {
                continue;
            }
            println!(
                "{:<12} {:>15.0} {:>15.0} {:>15.0}",
                ymd(**d * 86400),
                to_c,
                from_c,
                to_c - from_c
            );
        }
        let tot_to: f64 = days.iter().map(|x| x.1 .0).sum();
        let tot_from: f64 = days.iter().map(|x| x.1 .1).sum();
        println!("\nразом: на біржі ${:.0} · з бірж ${:.0} · нетто ${:.0}",
            tot_to, tot_from, tot_to - tot_from);
        let _ = File::create(format!("{bf}/flowdir-daily{sfx}.json")).unwrap().write_all(
            serde_json::to_string(&json!({"days": days.iter().map(|(d, (a, b))|
                json!({"day": **d, "date": ymd(**d * 86400), "toCex": a, "fromCex": b}))
                .collect::<Vec<_>>()})).unwrap().as_bytes());
    }

    // ── ЩО СТАЛОСЯ ПІСЛЯ: депозит на біржу → ціна ─────────────────────────────
    //
    // Питання просте: якщо ці адреси заводять актив на біржу, чи падає він після цього.
    // Але «впав після депозиту» саме по собі не означає нічого: у падаючому ринку ціна
    // падає після будь-якої випадкової години. Тому кожен горизонт порівнюється з
    // БАЗОВОЮ ЛІНІЄЮ — середньою зміною того самого токена за той самий проміжок,
    // порахованою по всьому ряду. Значення має лише РІЗНИЦЯ.
    //
    // Для старої мережі такий тест впливу не підтвердив (зростання передувало купівлям),
    // тож нульовий результат тут очікуваний і його треба показати чесно.
    let mut hourly: HashMap<String, Vec<(u64, f64)>> = HashMap::new();
    if let Ok(f) = File::open(format!("{bf}/token-prices-hourly.json")) {
        if let Ok(h) = serde_json::from_reader::<_, Value>(BufReader::new(f)) {
            let rootv = if h.get("tokens").is_some() { h["tokens"].clone() } else { h };
            if let Some(o) = rootv.as_object() {
                for (c, v) in o {
                    let mut pts: Vec<(u64, f64)> = v["prices"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default()
                        .iter()
                        .filter_map(|p| Some((p[0].as_u64()?, p[1].as_f64()?)))
                        .collect();
                    pts.sort_by_key(|p| p.0);
                    if pts.len() >= 100 {
                        hourly.insert(c.to_lowercase(), pts);
                    }
                }
            }
        }
    }
    eprintln!("[flowdir] годинних рядів: {} · подій ≥${:.0}: {}", hourly.len(), min_event, events.len());

    if !hourly.is_empty() && !events.is_empty() {
        const HORIZONS: [(&str, u64); 4] =
            [("+1г", 3600), ("+6г", 21600), ("+24г", 86400), ("+72г", 259200)];

        // базова лінія по кожному токену: середня зміна за горизонт по ВСЬОМУ ряду
        let mut base: HashMap<String, [f64; 4]> = HashMap::new();
        for (c, pts) in &hourly {
            let mut acc = [0f64; 4];
            let mut cnt = [0u32; 4];
            for (hi, (_, secs)) in HORIZONS.iter().enumerate() {
                let step = (*secs / 3600).max(1) as usize;
                for i in 0..pts.len().saturating_sub(step) {
                    let (p0, p1) = (pts[i].1, pts[i + step].1);
                    if p0 > 0.0 {
                        acc[hi] += 100.0 * (p1 - p0) / p0;
                        cnt[hi] += 1;
                    }
                }
            }
            let mut b = [0f64; 4];
            for i in 0..4 {
                b[i] = if cnt[i] > 0 { acc[i] / cnt[i] as f64 } else { 0.0 };
            }
            base.insert(c.clone(), b);
        }

        // зміна після кожної події
        let mut agg: HashMap<bool, ([f64; 4], [u32; 4], f64)> = HashMap::new();
        for (c, ts, usd, is_dep) in &events {
            let Some(pts) = hourly.get(c) else { continue };
            let Some(b) = base.get(c) else { continue };
            let i = pts.partition_point(|p| p.0 <= *ts);
            if i == 0 || i >= pts.len() {
                continue;
            }
            let p0 = pts[i - 1].1;
            if p0 <= 0.0 || pts[i - 1].0.abs_diff(*ts) > 7200 {
                continue; // ряд не покриває момент події
            }
            let e = agg.entry(*is_dep).or_insert(([0f64; 4], [0u32; 4], 0.0));
            e.2 += *usd;
            for (hi, (_, secs)) in HORIZONS.iter().enumerate() {
                let step = (*secs / 3600).max(1) as usize;
                if i - 1 + step < pts.len() {
                    let p1 = pts[i - 1 + step].1;
                    e.0[hi] += 100.0 * (p1 - p0) / p0 - b[hi]; // ЗВЕРХ базової лінії
                    e.1[hi] += 1;
                }
            }
        }

        println!("\n=== ЩО СТАЛОСЯ ПІСЛЯ (зверх звичайного руху того ж токена) ===");
        for (is_dep, label) in [(true, "ДЕПОЗИТ НА БІРЖУ"), (false, "ВИВЕДЕННЯ З БІРЖІ")] {
            let Some((sum, cnt, usd)) = agg.get(&is_dep) else {
                println!("{label}: подій із покриттям ціною немає");
                continue;
            };
            println!("\n{label} · подій {} · ${:.0}", cnt[2].max(cnt[0]), usd);
            for (hi, (name, _)) in HORIZONS.iter().enumerate() {
                if cnt[hi] == 0 {
                    continue;
                }
                let avg = sum[hi] / cnt[hi] as f64;
                // ±0.05 пп на годинних даних — шум, а не сигнал
                let verdict = if avg.abs() < 0.05 {
                    "нічого"
                } else if avg < 0.0 {
                    "ЦІНА ПАДАЄ"
                } else {
                    "ЦІНА РОСТЕ"
                };
                println!("  {name:<5} {avg:>7.3} пп · {verdict} (подій {})", cnt[hi]);
            }
        }
        println!("\nЧитати так: показано ВІДХИЛЕННЯ від звичайного руху токена за той самий");
        println!("проміжок. Нуль означає, що після операцій мережі не стається нічого особливого.");
    }

    let _ = File::create(format!("{bf}/flowdir{sfx}.json")).unwrap().write_all(
        serde_json::to_string(&json!({"sellers": n_sell, "buyers": n_buy, "makers": n_maker,
            "sellUsd": sell_usd, "buyUsd": buy_usd, "rows": rows,
            "byToken": toks.iter().map(|(s, (d, w))| json!({"sym": s, "toCexUsd": d,
                "fromCexUsd": w, "netUsd": d - w})).collect::<Vec<_>>()}))
        .unwrap()
        .as_bytes(),
    );
    db.save(&bf);
    eprintln!("[flowdir] збережено flowdir{sfx}.json");
}
