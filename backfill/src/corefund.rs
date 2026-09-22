//! corefund — звідки взявся капітал ядра і чи ядро має спільне джерело.
//!
//! Ядро визначили не за розміром однієї позиції, а за повторюваністю: 20 адрес тримають
//! 58% підтверджених $5.68 млрд. Питання, на яке відповідає цей бін: ці 20 — незалежні
//! кити, що випадково потрапили в один список, чи їх заправляли з тих самих рук.
//!
//! Відповідь дає СПІЛЬНЕ ДЖЕРЕЛО. Якщо кожен профінансований своєю біржею — це просто
//! багаті адреси. Якщо ж кілька членів ядра отримали ETH з однієї адреси, і та адреса не
//! біржа, — це вже спільна каса.
//!
//! Окремо тримаємо ПЕРШИЙ вхід: він показує, хто адресу створив, і його не можна плутати
//! з найбільшим внеском — заправити газом міг один, а капітал прислати зовсім інший.
//!
//! Читає txfull-{chain}.jsonl одним проходом (44 ГБ), тому все потрібне збирається за раз.
//! Вихід: dataset/backfill/corefund.json + патерни в базу.
//! Args: ROOT [файл-зі-списком]. Env: CHAIN(1) MIN_ETH(0.05) TOPN(25)
use backfill::known::{self, pat};
use backfill::patterns::{Finding, PatternDb};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, Write};

fn envn<T: std::str::FromStr>(k: &str, d: T) -> T {
    std::env::var(k).ok().and_then(|s| s.parse().ok()).unwrap_or(d)
}

/// Зібрати вхідний ETH для заданих адрес одним проходом по історії.
///
/// Винесено окремо, бо рівнів виявилось більше двох: над спільними джерелами стоїть ще
/// один шар тих самих адрес. Дублювати прохід під кожен рівень немає сенсу — логіка
/// однакова, різняться лише цілі.
fn scan_inputs(
    bf: &str,
    chain: u32,
    targets: &HashSet<String>,
    min_eth: f64,
) -> HashMap<String, HashMap<String, f64>> {
    let mut up: HashMap<String, HashMap<String, f64>> = HashMap::new();
    if targets.is_empty() {
        return up;
    }
    let Ok(f) = File::open(format!("{bf}/txfull-{chain}.jsonl")) else { return up };
    let mut cur = String::new();
    let mut seen: HashSet<u64> = HashSet::new();
    for line in BufReader::with_capacity(1 << 22, f).lines().map_while(Result::ok) {
        if !line.contains("\"k\":\"ext\"") && !line.contains("\"k\":\"int\"") {
            continue;
        }
        let Ok(t) = serde_json::from_str::<Value>(&line) else { continue };
        let to = t["t"].as_str().unwrap_or("").to_lowercase();
        if !targets.contains(&to) {
            continue;
        }
        // дедуп у межах гаманця: глобальна множина на 38 млн рядків з'їдала 11 ГБ
        let w = t["w"].as_str().unwrap_or("");
        if w != cur {
            cur = w.to_string();
            seen.clear();
        }
        let eth: f64 = t["v"].as_str().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0) / 1e18;
        if eth < min_eth {
            continue;
        }
        let from = t["f"].as_str().unwrap_or("").to_lowercase();
        if from.is_empty() || from == to {
            continue;
        }
        let mut h: u64 = 1469598103934665603;
        for s in [t["h"].as_str().unwrap_or(""), from.as_str()] {
            for b in s.as_bytes() {
                h ^= *b as u64;
                h = h.wrapping_mul(1099511628211)
            }
        }
        if !seen.insert(h) {
            continue;
        }
        *up.entry(to).or_default().entry(from).or_insert(0.0) += eth;
    }
    up
}

/// Часовий профіль конкретних ребер: коли саме потік ішов і в які роки.
///
/// Сума без часу нічого не каже про причинність. «Стара мережа фінансує нову» і «обидві
/// брали з одного джерела в різні епохи» дають однакові підсумки в ETH, але це різні
/// твердження. Так само взаємна пара: якщо туди і назад ішло одночасно — це обмін, а якщо
/// назад через рік — це повернення позики.
fn scan_edge_times(
    bf: &str,
    chain: u32,
    want: &HashSet<(String, String)>,
    min_eth: f64,
) -> HashMap<(String, String), (f64, u64, u64, HashMap<i32, f64>)> {
    let mut out: HashMap<(String, String), (f64, u64, u64, HashMap<i32, f64>)> = HashMap::new();
    if want.is_empty() {
        return out;
    }
    let Ok(f) = File::open(format!("{bf}/txfull-{chain}.jsonl")) else { return out };
    let mut cur = String::new();
    let mut seen: HashSet<u64> = HashSet::new();
    for line in BufReader::with_capacity(1 << 22, f).lines().map_while(Result::ok) {
        if !line.contains("\"k\":\"ext\"") && !line.contains("\"k\":\"int\"") {
            continue;
        }
        let Ok(t) = serde_json::from_str::<Value>(&line) else { continue };
        let from = t["f"].as_str().unwrap_or("").to_lowercase();
        let to = t["t"].as_str().unwrap_or("").to_lowercase();
        let key = (from, to);
        if !want.contains(&key) {
            continue;
        }
        let w = t["w"].as_str().unwrap_or("");
        if w != cur {
            cur = w.to_string();
            seen.clear();
        }
        let eth: f64 = t["v"].as_str().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0) / 1e18;
        if eth < min_eth {
            continue;
        }
        let mut h: u64 = 1469598103934665603;
        for b in t["h"].as_str().unwrap_or("").as_bytes() {
            h ^= *b as u64;
            h = h.wrapping_mul(1099511628211)
        }
        if !seen.insert(h) {
            continue;
        }
        let ts = t["ts"].as_u64().unwrap_or(0);
        // рік без календарної бібліотеки: 1970 + секунди / середня довжина року
        let year = 1970 + (ts as f64 / 31_556_952.0).floor() as i32;
        let e = out.entry(key).or_insert((0.0, u64::MAX, 0, HashMap::new()));
        e.0 += eth;
        if ts > 0 && ts < e.1 {
            e.1 = ts
        }
        if ts > e.2 {
            e.2 = ts
        }
        *e.3.entry(year).or_insert(0.0) += eth;
    }
    out
}

/// Токенні входи для заданих адрес: хто, скільки і коли передав конкретний актив.
///
/// Потрібне там, де ETH-трасування нічого не бачить. Три кити з великими позиціями
/// отримали майже нуль ETH (`0xd250b488` — 0.2 ETH при WLFI на $52 млн, `0xb805ef84` —
/// жодного при ENA на $127 млн). Купити на ринку так неможливо, отже актив прийшов
/// готовим — і слід лишився в токенних переказах, а не в ETH.
fn scan_token_inputs(
    bf: &str,
    chain: u32,
    targets: &HashSet<String>,
    sym_want: &str,
) -> HashMap<String, HashMap<String, (f64, u64, u64, u32)>> {
    let mut out: HashMap<String, HashMap<String, (f64, u64, u64, u32)>> = HashMap::new();
    if targets.is_empty() {
        return out;
    }
    let Ok(f) = File::open(format!("{bf}/txfull-{chain}.jsonl")) else { return out };
    let mut cur = String::new();
    let mut seen: HashSet<u64> = HashSet::new();
    let want_lc = sym_want.to_lowercase();
    for line in BufReader::with_capacity(1 << 22, f).lines().map_while(Result::ok) {
        if !line.contains("\"k\":\"tok\"") {
            continue;
        }
        let Ok(t) = serde_json::from_str::<Value>(&line) else { continue };
        let sym = t["sym"].as_str().unwrap_or("");
        if !want_lc.is_empty() && sym.to_lowercase() != want_lc {
            continue;
        }
        let to = t["t"].as_str().unwrap_or("").to_lowercase();
        if !targets.contains(&to) {
            continue;
        }
        let w = t["w"].as_str().unwrap_or("");
        if w != cur {
            cur = w.to_string();
            seen.clear();
        }
        let mut h: u64 = 1469598103934665603;
        for s in [t["h"].as_str().unwrap_or(""), t["v"].as_str().unwrap_or("")] {
            for b in s.as_bytes() {
                h ^= *b as u64;
                h = h.wrapping_mul(1099511628211)
            }
        }
        if !seen.insert(h) {
            continue;
        }
        // decimals беремо з самого рядка: у різних токенів вони різні, і 18 за
        // замовчуванням дало б для USDC помилку в мільйон разів
        let dec: i32 = t["dec"].as_str().and_then(|s| s.parse().ok()).unwrap_or(18);
        let amt: f64 =
            t["v"].as_str().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0) / 10f64.powi(dec);
        if amt <= 0.0 {
            continue;
        }
        let from = t["f"].as_str().unwrap_or("").to_lowercase();
        if from.is_empty() || from == to {
            continue;
        }
        let ts = t["ts"].as_u64().unwrap_or(0);
        let e = out.entry(to).or_default().entry(from).or_insert((0.0, u64::MAX, 0, 0));
        e.0 += amt;
        if ts > 0 && ts < e.1 {
            e.1 = ts
        }
        if ts > e.2 {
            e.2 = ts
        }
        e.3 += 1;
    }
    out
}

/// Дата з мітки часу — щоб не тягнути календарну залежність у крейт.
fn ymd(ts: u64) -> String {
    if ts == 0 || ts == u64::MAX {
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

#[derive(Default)]
struct In {
    first_ts: u64,
    first_from: String,
    first_eth: f64,
    total_eth: f64,
    from_sum: HashMap<String, f64>,
    n: u64,
}

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let bf = format!("{root}/dataset/backfill");
    let chain: u32 = envn("CHAIN", 1);
    let min_eth: f64 = envn("MIN_ETH", 0.05);
    let topn: usize = envn("TOPN", 25);

    let mut db = PatternDb::load(&bf);

    // мітки — щоб відрізнити біржу від приватного джерела
    let mut labels: HashMap<String, String> = HashMap::new();
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
                if !s.trim().is_empty() {
                    labels.insert(a.to_lowercase(), s);
                }
            }
        }
    }

    // ядро: або зі списку, або з бази — ті, кого holdercheck назвав китами
    let core: Vec<String> = match std::env::args().nth(2) {
        Some(p) => BufReader::new(File::open(p).expect("список"))
            .lines()
            .map_while(Result::ok)
            .map(|l| l.trim().to_lowercase())
            .filter(|l| l.len() == 42)
            .collect(),
        None => {
            let mut v: Vec<String> = db
                .map
                .iter()
                .filter(|(_, r)| {
                    r.iter().any(|x| x.pat == "MULTI-ASSET-WHALE" || x.pat == "WHALE-POSITION")
                })
                .map(|(a, _)| a.clone())
                .collect();
            v.sort();
            v
        }
    };
    let core_set: HashSet<&str> = core.iter().map(|s| s.as_str()).collect();
    eprintln!("[corefund] ядро: {} адрес", core.len());

    // ── ТОКЕННИЙ РЕЖИМ ────────────────────────────────────────────────────────
    // Вмикається через TOKEN=SYM. Виходить раніше за ETH-аналіз: там, де актив прийшов
    // готовим, ETH-трасування показує порожнечу й веде хибним слідом.
    if let Ok(sym) = std::env::var("TOKEN") {
        let hops: usize = envn("TOK_HOPS", 3);
        let mut targets: HashSet<String> = core.iter().cloned().collect();
        let mut walked: HashSet<String> = targets.clone();
        println!("\n=== ЗВІДКИ ПРИЙШОВ {sym} ===");

        for hop in 1..=hops {
            if targets.is_empty() {
                break;
            }
            eprintln!("[corefund] {sym}, хоп {hop}: {} адрес", targets.len());
            let got = scan_token_inputs(&bf, chain, &targets, &sym);
            let mut next: HashSet<String> = HashSet::new();

            for to in &targets {
                let Some(m) = got.get(to) else {
                    println!("  хоп {hop} · {} ← {sym} не надходив", &to[..20.min(to.len())]);
                    continue;
                };
                let mut v: Vec<(&String, &(f64, u64, u64, u32))> = m.iter().collect();
                v.sort_by(|a, b| b.1 .0.partial_cmp(&a.1 .0).unwrap());
                let tot: f64 = v.iter().map(|x| x.1 .0).sum();
                println!(
                    "  хоп {hop} · {} ← {:.0} {sym} від {} адрес",
                    &to[..20.min(to.len())],
                    tot,
                    v.len()
                );
                for (from, (amt, t0, t1, n)) in v.iter().take(5) {
                    let lbl = labels.get(*from).cloned().unwrap_or_default();
                    let kind = if known::is_cex_label(&lbl) || db.has(from, pat::CEX_LABELED) {
                        "БІРЖА"
                    } else if db.has(from, pat::CONTRACT) {
                        "контракт"
                    } else if from.starts_with(wr_store::addrs::zero()) {
                        "МІНТ"
                    } else {
                        "EOA"
                    };
                    let member = if db.has(from, "GRP-NEW") {
                        " [нова група]"
                    } else if db.has(from, "GRP-6864") {
                        " [стара мережа]"
                    } else {
                        ""
                    };
                    println!(
                        "      {} {:>16.0} ×{:<4} {} → {}  {}{} {}",
                        &from[..20.min(from.len())],
                        amt,
                        n,
                        ymd(*t0),
                        ymd(*t1),
                        kind,
                        member,
                        lbl.chars().take(22).collect::<String>()
                    );
                    // мінт означає, що далі йти нікуди: токен народився тут
                    if kind == "EOA" && walked.insert((*from).clone()) {
                        next.insert((*from).clone());
                    }
                    if kind == "МІНТ" {
                        db.add(
                            to,
                            Finding::new(pat::ALLOCATION, "corefund").evk("minted", &sym, *amt),
                        );
                    }
                    // Пишемо в базу кожен бік ребра, а не лише рідкісний мінт. Одна
                    // передача сама по собі мало що каже, але коли та сама адреса
                    // спливе постачальником у третьому активі — це вже видно тільки
                    // з накопиченого, і відновити його заднім числом нізвідки.
                    db.add(
                        from,
                        Finding::new("TOKEN-SOURCE", "corefund")
                            .evk("sent", &sym, *amt)
                            .evk("to", &to[..10], *n as f64),
                    );
                    db.add(
                        to,
                        Finding::new("TOKEN-RECEIVED", "corefund")
                            .evk("got", &sym, *amt)
                            .evk("hop", &sym, hop as f64),
                    );
                }
            }
            targets = next;
        }
        db.save(&bf);
        return;
    }

    // ── один прохід по історії ────────────────────────────────────────────────
    let mut inp: HashMap<String, In> = HashMap::new();
    let f = File::open(format!("{bf}/txfull-{chain}.jsonl")).expect("txfull");
    let mut n = 0u64;
    let mut cur_w = String::new();
    let mut seen: HashSet<u64> = HashSet::new();

    for line in BufReader::with_capacity(1 << 22, f).lines().map_while(Result::ok) {
        n += 1;
        if n % 5_000_000 == 0 {
            eprintln!("    ...{n} рядків");
        }
        // ETH рахуємо ЛИШЕ з ext/int: у токенних рядках теж є поле v, і ділення їх
        // на 1e18 колись дало «19.8 трлн ETH»
        if !line.contains("\"k\":\"ext\"") && !line.contains("\"k\":\"int\"") {
            continue;
        }
        let Ok(t) = serde_json::from_str::<Value>(&line) else { continue };
        if !known::is_eth_row(t["k"].as_str().unwrap_or("")) {
            continue;
        }
        let to = t["t"].as_str().unwrap_or("").to_lowercase();
        if !core_set.contains(to.as_str()) {
            continue;
        }
        // дедуп у межах гаманця: глобальна множина на 38 млн рядків з'їдала 11 ГБ
        let w = t["w"].as_str().unwrap_or("");
        if w != cur_w {
            cur_w = w.to_string();
            seen.clear();
        }
        let eth: f64 = t["v"].as_str().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0) / 1e18;
        if eth < min_eth {
            continue;
        }
        let from = t["f"].as_str().unwrap_or("").to_lowercase();
        if from.is_empty() || from == to {
            continue;
        }
        let mut h: u64 = 1469598103934665603;
        for s in [t["h"].as_str().unwrap_or(""), from.as_str(), t["v"].as_str().unwrap_or("")] {
            for b in s.as_bytes() {
                h ^= *b as u64;
                h = h.wrapping_mul(1099511628211)
            }
        }
        if !seen.insert(h) {
            continue;
        }
        let ts = t["ts"].as_u64().unwrap_or(0);
        let e = inp.entry(to).or_default();
        if e.first_ts == 0 || ts < e.first_ts {
            e.first_ts = ts;
            e.first_from = from.clone();
            e.first_eth = eth;
        }
        e.total_eth += eth;
        e.n += 1;
        *e.from_sum.entry(from).or_insert(0.0) += eth;
    }
    eprintln!("[corefund] рядків {n} · адрес із входами {}", inp.len());

    // ── хто кого фінансував ───────────────────────────────────────────────────
    // адреса-джерело -> скільки РІЗНИХ членів ядра вона заправила
    let mut funder_reach: HashMap<String, (usize, f64, Vec<String>)> = HashMap::new();
    for (a, v) in &inp {
        for (src, eth) in &v.from_sum {
            let e = funder_reach.entry(src.clone()).or_insert((0, 0.0, Vec::new()));
            e.0 += 1;
            e.1 += *eth;
            if e.2.len() < 12 {
                e.2.push(a.clone());
            }
        }
    }

    let mut rows: Vec<(&String, &In)> = inp.iter().collect();
    rows.sort_by(|a, b| b.1.total_eth.partial_cmp(&a.1.total_eth).unwrap());

    println!("\n=== ЧИМ ЗАПРАВЛЕНО ЯДРО (вхідний ETH) ===");
    println!("{:<44} {:>13} {:>6}  {}", "АДРЕСА", "ETH УСЬОГО", "ВХОДІВ", "ГОЛОВНЕ ДЖЕРЕЛО");
    let mut out_rows: Vec<Value> = Vec::new();
    for (a, v) in rows.iter().take(topn) {
        let mut srcs: Vec<(&String, &f64)> = v.from_sum.iter().collect();
        srcs.sort_by(|x, y| y.1.partial_cmp(x.1).unwrap());
        let (top_src, top_eth) = srcs.first().map(|(s, e)| (s.as_str(), **e)).unwrap_or(("", 0.0));
        let lbl = labels.get(top_src).cloned().unwrap_or_default();
        let kind = if known::is_cex_label(&lbl) {
            "БІРЖА"
        } else if db.has(top_src, pat::CEX_LABELED) {
            "БІРЖА"
        } else if core_set.contains(top_src) {
            "ЧЛЕН ЯДРА"
        } else {
            "приватне"
        };
        println!(
            "{a} {:>13.1} {:>6}  {} {:.0} ETH {} {}",
            v.total_eth,
            v.n,
            &top_src[..12.min(top_src.len())],
            top_eth,
            kind,
            lbl.chars().take(24).collect::<String>()
        );
        out_rows.push(json!({"addr": a, "totalEth": v.total_eth, "nIn": v.n,
            "firstFrom": v.first_from, "firstTs": v.first_ts, "firstEth": v.first_eth,
            "topSrc": top_src, "topSrcEth": top_eth, "topSrcKind": kind, "topSrcLabel": lbl,
            "sources": srcs.len()}));

        if kind == "БІРЖА" {
            db.add(a, Finding::new("FUNDED-BY-CEX", "corefund").ev("eth", top_eth));
        }
    }

    // ── СПІЛЬНІ ДЖЕРЕЛА: те, заради чого все й рахувалось ─────────────────────
    let mut shared: Vec<(&String, &(usize, f64, Vec<String>))> =
        funder_reach.iter().filter(|(_, v)| v.0 >= 2).collect();
    shared.sort_by(|a, b| b.1 .0.cmp(&a.1 .0).then(b.1 .1.partial_cmp(&a.1 .1).unwrap()));

    println!("\n=== СПІЛЬНІ ДЖЕРЕЛА (заправили ≥2 членів ядра) ===");
    if shared.is_empty() {
        println!("  немає — кожен член ядра має власне джерело, спільної каси не видно");
    }
    let mut priv_shared = 0usize;
    for (src, (cnt, eth, who)) in shared.iter().take(25) {
        let lbl = labels.get(*src).cloned().unwrap_or_default();
        let is_cex = known::is_cex_label(&lbl) || db.has(src, pat::CEX_LABELED);
        // Роутер DEX проходить через себе кошти всіх підряд, тож «заправив 4 членів ядра»
        // для нього нічого не означає. Перший прогін підняв так WETH, 1inch і CoW —
        // спільним джерелом їх називати не можна.
        let is_contract = db.has(src, pat::CONTRACT);
        if is_contract {
            println!(
                "  {} → {} членів · {:.0} ETH · КОНТРАКТ (роутер/протокол, не джерело) {}",
                &src[..20.min(src.len())],
                cnt,
                eth,
                lbl.chars().take(30).collect::<String>()
            );
            continue;
        }
        if !is_cex {
            priv_shared += 1;
            // приватна адреса, що заправила кількох китів, — кандидат у casу групи
            db.add(
                src,
                Finding::new("SHARED-FUNDER", "corefund")
                    .ev("reach", *cnt as f64)
                    .ev("eth", *eth),
            );
        }
        println!(
            "  {} → {} членів · {:.0} ETH · {} {}",
            &src[..20.min(src.len())],
            cnt,
            eth,
            if is_cex { "БІРЖА" } else { "ПРИВАТНЕ" },
            lbl.chars().take(30).collect::<String>()
        );
        if !is_cex {
            println!("      кого: {}", who.iter().map(|a| &a[..10]).collect::<Vec<_>>().join(" "));
        }
    }
    println!("\nприватних спільних джерел: {priv_shared} із {} усіх спільних", shared.len());

    // ── РІВЕНЬ 2: хто заправляв самі спільні джерела ──────────────────────────
    // Спільне джерело — ще не вершина. Якщо воно саме бере з біржі, то це просто
    // роздавач; якщо ж над ним стоїть ще одна приватна адреса — ієрархія глибша, ніж
    // здавалось, і шукати треба її. Другий прохід по історії робимо лише для тих
    // небагатьох адрес, що пройшли відсів, — повний прохід коштує 124 млн рядків.
    let lvl2: HashSet<String> = shared
        .iter()
        .filter(|(s, v)| {
            v.0 >= 3
                && !db.has(s, pat::CONTRACT)
                && !db.has(s, pat::CEX_LABELED)
                && !known::is_cex_label(labels.get(*s).map(|x| x.as_str()).unwrap_or(""))
        })
        .map(|(s, _)| (*s).clone())
        .collect();

    // Ідемо вгору, поки шар дає нові приватні адреси. Зупиняємось на біржі чи контракті:
    // вище них ланцюг обривається — там уже не видно, хто розпоряджається коштами.
    let levels: usize = envn("LEVELS", 3);
    let up_min: f64 = envn("UP_MIN_ETH", 50_000.0);
    let mut cur_targets = lvl2;
    let mut climbed: HashSet<String> = cur_targets.iter().cloned().collect();
    // всі побачені ребра — щоб потім спитати, чи потік справді йде згори вниз
    let mut edges: HashMap<(String, String), f64> = HashMap::new();

    for lvl in 2..=levels {
        if cur_targets.is_empty() {
            break;
        }
        eprintln!("[corefund] рівень {lvl}: шукаємо фандерів для {} адрес", cur_targets.len());
        let up = scan_inputs(&bf, chain, &cur_targets, min_eth);
        println!("\n=== РІВЕНЬ {lvl}: ХТО СТОЇТЬ ВИЩЕ ===");
        let mut next: HashSet<String> = HashSet::new();

        for src in &cur_targets {
            let Some(m) = up.get(src) else {
                println!("  {} ← входів немає (історія не зібрана)", &src[..20.min(src.len())]);
                continue;
            };
            let mut v: Vec<(&String, &f64)> = m.iter().collect();
            v.sort_by(|a, b| b.1.partial_cmp(a.1).unwrap());
            let tot: f64 = v.iter().map(|x| *x.1).sum();
            println!("  {} ← {:.0} ETH від {} джерел:", &src[..20.min(src.len())], tot, v.len());
            for (a, eth) in v.iter().take(4) {
                edges.insert(((*a).clone(), src.clone()), **eth);
                let lbl = labels.get(*a).cloned().unwrap_or_default();
                let is_cex = known::is_cex_label(&lbl) || db.has(a, pat::CEX_LABELED);
                let is_con = db.has(a, pat::CONTRACT);
                let kind = if is_cex {
                    "БІРЖА"
                } else if is_con {
                    "контракт"
                } else {
                    "ПРИВАТНЕ"
                };
                println!(
                    "      {} {:>10.0} ETH {} {}",
                    &a[..20.min(a.len())],
                    eth,
                    kind,
                    lbl.chars().take(26).collect::<String>()
                );
                if !is_cex && !is_con && **eth > 1000.0 {
                    db.add(
                        a,
                        Finding::new("UPSTREAM-FUNDER", "corefund")
                            .ev("eth", **eth)
                            .ev("level", lvl as f64),
                    );
                    // вище підіймаємо лише великі приватні вузли, і кожен — один раз:
                    // інакше обхід розповзеться на тисячі дрібних адрес
                    if **eth >= up_min && climbed.insert((*a).clone()) {
                        next.insert((*a).clone());
                    }
                }
            }
        }
        cur_targets = next;
    }

    // ── ЧИ ЦЕ ВЗАГАЛІ ІЄРАРХІЯ ────────────────────────────────────────────────
    // Підйом «угору» мовчазно припускає, що потік має напрям: гроші спускаються від
    // казни до виконавців. Але якщо A шле B, і B шле A зіставну суму, то ніякого «вище»
    // немає — це обмін між рівними, і називати таку адресу вершиною було б вигадкою.
    {
        let mut pairs: Vec<(&String, &String, f64, f64)> = Vec::new();
        for ((f, t), eth) in &edges {
            if f >= t {
                continue; // пару беремо один раз
            }
            if let Some(back) = edges.get(&((*t).clone(), (*f).clone())) {
                pairs.push((f, t, *eth, *back));
            }
        }
        pairs.sort_by(|a, b| (b.2 + b.3).partial_cmp(&(a.2 + a.3)).unwrap());

        println!("\n=== ВЗАЄМНІ ПОТОКИ (обмін, а не підпорядкування) ===");
        if pairs.is_empty() {
            println!("  немає — потік односпрямований, ієрархія справжня");
        }
        for (f, t, ab, ba) in pairs.iter().take(15) {
            let ratio = ab.min(*ba) / ab.max(*ba).max(1e-9);
            println!(
                "  {} ⇄ {} · {:.0} / {:.0} ETH · симетрія {:.0}%",
                &f[..14.min(f.len())],
                &t[..14.min(t.len())],
                ab,
                ba,
                100.0 * ratio
            );
            // симетрична пара — ознака замкненого контуру, а не роздачі згори
            if ratio >= 0.25 {
                for a in [f, t] {
                    db.add(a, Finding::new("CIRCULAR-FLOW", "corefund").ev("ethPair", ab + ba));
                }
            }
        }
        println!("\nвзаємних пар: {}", pairs.len());
    }

    // ── КОЛИ саме йшов потік ──────────────────────────────────────────────────
    // Найважливіше питання лишалось без відповіді: підсумок у ETH однаковий і тоді, коли
    // одна мережа фінансує іншу, і тоді, коли обидві просто брали з того самого джерела
    // в різні роки. Розрізняє їх лише час.
    {
        let mut want: HashSet<(String, String)> = HashSet::new();
        let mut big: Vec<(&(String, String), &f64)> = edges.iter().collect();
        big.sort_by(|a, b| b.1.partial_cmp(a.1).unwrap());
        for (k, _) in big.iter().take(14) {
            want.insert((*k).clone());
            want.insert((k.1.clone(), k.0.clone())); // і зворотний напрям
        }
        eprintln!("[corefund] часовий профіль для {} ребер", want.len());
        let times = scan_edge_times(&bf, chain, &want, min_eth);

        let mut rows: Vec<(&(String, String), &(f64, u64, u64, HashMap<i32, f64>))> =
            times.iter().filter(|(_, v)| v.0 > 0.0).collect();
        rows.sort_by(|a, b| b.1 .0.partial_cmp(&a.1 .0).unwrap());

        println!("\n=== ХРОНОЛОГІЯ ГОЛОВНИХ РЕБЕР ===");
        println!("{:<16} {:<16} {:>11}  {:<12} {:<12} {}", "ВІД", "ДО", "ETH", "ПЕРШИЙ", "ОСТАННІЙ", "ПО РОКАХ");
        for ((f, t), (eth, t0, t1, years)) in rows.iter().take(20) {
            let mut ys: Vec<(&i32, &f64)> = years.iter().collect();
            ys.sort_by_key(|x| *x.0);
            let byy: Vec<String> = ys
                .iter()
                .map(|(y, e)| {
                    if **e >= 1000.0 {
                        format!("{y}:{:.0}k", **e / 1000.0)
                    } else {
                        format!("{y}:{:.0}", **e)
                    }
                })
                .collect();
            println!(
                "{:<16} {:<16} {:>11.0}  {:<12} {:<12} {}",
                &f[..14.min(f.len())],
                &t[..14.min(t.len())],
                eth,
                ymd(*t0),
                ymd(*t1),
                byy.join(" ")
            );
        }
    }

    let _ = File::create(format!("{bf}/corefund.json")).unwrap().write_all(
        serde_json::to_string(&json!({"core": core.len(), "withInputs": inp.len(),
            "rows": out_rows,
            "shared": shared.iter().take(60).map(|(s, (c, e, w))| json!({"src": s, "reach": c,
                "eth": e, "label": labels.get(*s), "members": w})).collect::<Vec<_>>()}))
        .unwrap()
        .as_bytes(),
    );
    db.save(&bf);
    eprintln!("[corefund] збережено corefund.json");
}
