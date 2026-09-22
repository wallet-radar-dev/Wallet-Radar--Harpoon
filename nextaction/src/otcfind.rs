//! otcfind — пошук OTC-угод у повній транзакційній базі.
//!
//! Позабіржова угода на ланцюгу виглядає як ЗУСТРІЧНА ПАРА переказів між двома звичайними
//! гаманцями: A віддає актив X, B у відповідь віддає ETH або стейбл на ЗІСТАВНУ суму, і все це
//! в межах кількох годин. Ні пулу, ні біржі в ланцюжку нема — саме тому на агрегатах це
//! невидиме: там лишається тільки підсумок обміну з контрагентом.
//!
//! ТРИ ФОРМИ, бо угоди бувають різні й одна схема їх не ловить:
//!   1) АТОМАРНА  — обидві ноги в одній транзакції (через ескроу-контракт теж), спільний хеш
//!   2) ЗУСТРІЧНА — A віддав актив, B у відповідь оплату; різні активи, зіставні суми
//!   3) ТРЕТЯ СТОРОНА — оплата приходить не від отримувача активу, а від пов'язаної з ним адреси
//! Оплатою може бути і ETH/стейбл, і ІНШИЙ ТОКЕН (обмін актив-на-актив теж угода).
//! Категорії за силою: ДОКАЗ (одна tx, або ≤5% і ≤1 год) · КАНДИДАТ (≤TOL і ≤WIN) ·
//! СЛАБКИЙ (розбіжність до 25% — типова знижка OTC — або до 48 год).
//!
//! Побічно це РОЗРІЗНЯЄ дві моделі структури: у OTC-служби одна адреса робить парні розрахунки
//! з БАГАТЬМА різними контрагентами (це бізнес), у координованої мережі переважає односторонній
//! рух усередину свого кола.
//!
//! Вхід: txfull-<chain>.jsonl, token-prices-hist.json, labels-cache, wallet-kind.
//! Вихід: dataset/backfill/otc-<chain>.json. Args: ROOT. Env: CHAIN(1) MIN_USD(25000) WIN_H(12) TOL(0.10)
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};

const STABLES: [&str; 8] = ["USDT", "USDC", "DAI", "TUSD", "USDE", "FDUSD", "PYUSD", "RLUSD"];

/// Обгортки, розписки й похідні стейкінгу. Обмін між ними НЕ є угодою: WETH↔aEthWETH це вклад
/// в Aave, wstETH↔WETH — стейкінг у Lido, ETH↔WETH — обгортання. Перший прогін без цього фільтра
/// дав 483 тисячі «угод», де топ складали саме такі протокольні операції.
fn is_wrapper(sym: &str) -> bool {
    let s = sym.to_uppercase();
    s == "WETH" || s == "ETH" && false
        || s.starts_with("AETH") || s.starts_with("AARB") || s.starts_with("AOPT") || s.starts_with("ABAS")
        || s.starts_with("WST") || s.starts_with("STETH") || s.starts_with("CBETH") || s.starts_with("RETH")
        || s.starts_with("WEETH") || s.starts_with("EETH") || s.starts_with("OSETH") || s.starts_with("SFRX")
        || s.starts_with("CUSD") || s.starts_with("CDAI") || s.starts_with("CETH") || s.starts_with("SUSD")
        || s.starts_with("UNI-V") || s.starts_with("SLP") || s.ends_with("-LP")
}
/// одна родина активів: обмін у межах родини — не угода, а конвертація
fn same_family(a: &str, b: &str) -> bool {
    let fam = |s: &str| { let u = s.to_uppercase();
        if u.contains("BTC") { "BTC" } else if u.contains("ETH") { "ETH" }
        else if u.contains("USD") || u == "DAI" || u == "FRAX" { "USD" } else { "OTHER" } };
    let (fa, fb) = (fam(a), fam(b));
    fa == fb && fa != "OTHER"
}

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let chain: u32 = std::env::var("CHAIN").ok().and_then(|s| s.parse().ok()).unwrap_or(1);
    let min_usd: f64 = std::env::var("MIN_USD").ok().and_then(|s| s.parse().ok()).unwrap_or(25_000.0);
    let win: u64 = std::env::var("WIN_H").ok().and_then(|s| s.parse().ok()).unwrap_or(12) * 3600;
    let tol: f64 = std::env::var("TOL").ok().and_then(|s| s.parse().ok()).unwrap_or(0.10);
    let bf = format!("{root}/dataset/backfill");

    // ціни: контракт -> день -> ціна (історичні), плюс ETH
    let hp: Value = serde_json::from_reader(BufReader::new(File::open(format!("{bf}/token-prices-hist.json")).expect("token-prices-hist"))).unwrap();
    let mut hist: HashMap<String, HashMap<i64, f64>> = HashMap::new();
    for (c, t) in hp["tokens"].as_object().cloned().unwrap_or_default() {
        let m: HashMap<i64, f64> = t["prices"].as_array().cloned().unwrap_or_default().iter()
            .filter_map(|p| Some(((p[0].as_u64()? / 86400) as i64, p[1].as_f64()?))).collect();
        hist.insert(c.to_lowercase(), m);
    }
    let cur: HashMap<String, f64> = serde_json::from_reader::<_, Value>(BufReader::new(File::open(format!("{bf}/token-prices.json")).unwrap())).ok()
        .and_then(|v| v["tokens"].as_object().map(|m| m.iter().filter_map(|(k, x)| x["usd"].as_f64().map(|u| (k.to_lowercase(), u))).collect())).unwrap_or_default();
    let eth_hist = hist.get("eth").cloned().unwrap_or_default();
    let price = |tok: &str, day: i64| -> f64 {
        if tok.is_empty() { return eth_hist.get(&day).copied().unwrap_or(0.0); }
        hist.get(tok).and_then(|m| m.get(&day)).copied()
            .or_else(|| cur.get(tok).copied()).unwrap_or(0.0)
    };

    // біржі та контракти — не сторони OTC
    let lbl: serde_json::Map<String, Value> = serde_json::from_reader(BufReader::new(File::open(format!("{root}/dataset/labels-cache.json")).unwrap())).unwrap_or_default();
    let mut kind: HashMap<String, String> = HashMap::new();
    if let Ok(v) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(File::open(format!("{bf}/wallet-kind.json")).unwrap())) {
        for (a, k) in &v { kind.insert(a.to_lowercase(), k.as_str().unwrap_or("").to_string()); }
    }
    let coded: std::collections::HashSet<String> = serde_json::from_reader::<_, Value>(BufReader::new(File::open(format!("{bf}/eth-balances.json")).unwrap()))
        .ok().and_then(|v| v["wallets"].as_object().map(|m| m.iter().filter(|(_, x)| x["code"] == json!(true)).map(|(k, _)| k.to_lowercase()).collect())).unwrap_or_default();
    // окремо перевірені контракти (codecheck) — закриває дірку, через яку контракт із нульовим
    // балансом проходив як «звичайний гаманець» і подавався як OTC-служба з 60 контрагентами
    let flags: HashMap<String, bool> = serde_json::from_reader(BufReader::new(File::open(format!("{bf}/contract-flags.json")).unwrap_or_else(|_| File::open("/dev/null").unwrap()))).unwrap_or_default();
    let bad = |a: &str| a.is_empty() || a.len() != 42 || coded.contains(a)
        || flags.get(a).copied().unwrap_or(false)
        || matches!(kind.get(a).map(|s| s.as_str()), Some("contract") | Some("exchange") | Some("otc"))
        || lbl.get(a).and_then(|x| x["name"].as_str()).map(|n| { let l = n.to_lowercase();
            ["binance","coinbase","okx","kraken","bybit","kucoin","huobi","exchange","router","pool","bridge","uniswap","deposit"].iter().any(|t| l.contains(t)) }).unwrap_or(false);

    // один прохід: лишаємо тільки великі перекази між НЕ-сервісними адресами
    #[derive(Clone)]
    struct Leg { ts: u64, from: String, to: String, sym: String, usd: f64, hash: String }
    let mut legs: HashMap<(String, String), Vec<Leg>> = HashMap::new();   // ключ = впорядкована пара
    let mut by_hash: HashMap<String, Vec<Leg>> = HashMap::new();          // ноги однієї транзакції
    let mut inbox: HashMap<String, Vec<Leg>> = HashMap::new();            // що приходило на адресу
    let (mut rows, mut kept) = (0u64, 0u64);
    for line in BufReader::with_capacity(1 << 22, File::open(format!("{bf}/txfull-{chain}.jsonl")).expect("txfull")).lines().map_while(Result::ok) {
        rows += 1;
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        if v["err"].as_str() == Some("1") { continue; }
        let (f, t) = (v["f"].as_str().unwrap_or("").to_string(), v["t"].as_str().unwrap_or("").to_string());
        if f.is_empty() || t.is_empty() || f == t { continue; }
        let both_eoa = !bad(&f) && !bad(&t);
        let ts = v["ts"].as_u64().unwrap_or(0);
        if ts == 0 { continue; }
        let day = (ts / 86400) as i64;
        let k = v["k"].as_str().unwrap_or("");
        let (sym, usd) = if k == "tok" {
            let tok = v["tok"].as_str().unwrap_or("").to_lowercase();
            let dec: i32 = v["dec"].as_str().and_then(|s| s.parse().ok()).unwrap_or(18);
            let raw: f64 = v["v"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0.0);
            (v["sym"].as_str().unwrap_or("?").to_string(), raw / 10f64.powi(dec) * price(&tok, day))
        } else {
            let raw: f64 = v["v"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0.0);
            ("ETH".to_string(), raw / 1e18 * price("", day))
        };
        if usd < min_usd { continue; }
        kept += 1;
        let leg = Leg { ts, from: f.clone(), to: t.clone(), sym, usd, hash: v["h"].as_str().unwrap_or("").to_string() };
        by_hash.entry(leg.hash.clone()).or_default().push(leg.clone());       // для атомарних
        if !leg.hash.is_empty() { inbox.entry(t.clone()).or_default().push(leg.clone()); }  // для третьої сторони
        if both_eoa {
            let key = if f < t { (f.clone(), t.clone()) } else { (t.clone(), f.clone()) };
            legs.entry(key).or_default().push(leg);
        }
        if rows % 10_000_000 == 0 { eprintln!("[otcfind] {rows} рядків, великих переказів {kept}, пар {}", legs.len()); }
    }
    eprintln!("[otcfind] опрацьовано {rows} рядків · великих переказів {kept} · пар адрес {}", legs.len());

    // пошук зустрічних пар
    let is_money = |s: &str| s == "ETH" || s == "WETH" || STABLES.contains(&s.to_uppercase().as_str());
    let mut deals: Vec<Value> = Vec::new();
    for ((a, b), v) in legs.iter_mut() {
        if v.len() < 2 { continue; }
        v.sort_by_key(|l| l.ts);
        for i in 0..v.len() {
            for j in (i + 1)..v.len() {
                let (x, y) = (&v[i], &v[j]);
                if y.ts - x.ts > win { break; }
                if x.from == y.from { continue; }                       // потрібен ЗУСТРІЧНИЙ напрям
                if x.sym == y.sym { continue; }                          // однаковий актив — це не угода
                if is_wrapper(&x.sym) || is_wrapper(&y.sym) { continue; } // вклад/стейкінг, не торг
                if same_family(&x.sym, &y.sym) { continue; }              // конвертація в межах родини
                let money_leg = is_money(&x.sym) || is_money(&y.sym);      // обмін актив-на-актив теж угода
                let (hi, lo) = if x.usd > y.usd { (x.usd, y.usd) } else { (y.usd, x.usd) };
                if hi <= 0.0 { continue; }
                let diff = (hi - lo) / hi;
                let same_tx = x.hash == y.hash && !x.hash.is_empty();
                let dt = y.ts - x.ts;
                let class = if same_tx || (diff <= 0.05 && dt <= 3600) { "ДОКАЗ" }
                    else if diff <= tol { "КАНДИДАТ" }
                    else if diff <= 0.25 && dt <= 48 * 3600 { "СЛАБКИЙ" }     // типова знижка OTC
                    else { continue };
                deals.push(json!({"a": a, "b": b, "class": class, "sameTx": same_tx, "form": "зустрічна", "money": money_leg,
                    "gapMin": dt / 60, "diffPct": diff * 100.0,
                    "leg1": {"ts": x.ts, "from": x.from, "sym": x.sym, "usd": x.usd},
                    "leg2": {"ts": y.ts, "from": y.from, "sym": y.sym, "usd": y.usd},
                    "usd": lo}));
            }
        }
    }
    // ФОРМА 1 ПРИБРАНА. Вона допускала контракт-посередник — і через це в результат потрапив
    // увесь DeFi: вклади в Aave, стейкінг Lido, свопи через роутер. Позабіржова угода за
    // визначенням іде БЕЗ ринкового майданчика, тож лишаємо лише прямі перекази між EOA.
    #[allow(unreachable_code)]
    for (h, v) in std::iter::empty::<(&String, &Vec<Leg>)>() {
        if h.is_empty() || v.len() < 2 { continue; }
        for i in 0..v.len() {
            for j in (i + 1)..v.len() {
                let (x, y) = (&v[i], &v[j]);
                if x.sym == y.sym { continue; }
                // сторона угоди: віддала один актив і в тій самій tx отримала інший
                let party = if !bad(&x.from) && x.from == y.to { Some((x.from.clone(), y.from.clone())) }
                    else if !bad(&y.from) && y.from == x.to { Some((y.from.clone(), x.from.clone())) }
                    else { None };
                let Some((p, cp)) = party else { continue };
                let (hi, lo) = if x.usd > y.usd { (x.usd, y.usd) } else { (y.usd, x.usd) };
                if hi <= 0.0 { continue; }
                deals.push(json!({"a": p, "b": cp, "class": "ДОКАЗ", "sameTx": true, "form": "атомарна",
                    "money": is_money(&x.sym) || is_money(&y.sym), "gapMin": 0, "diffPct": (hi - lo) / hi * 100.0,
                    "leg1": {"ts": x.ts, "from": x.from, "sym": x.sym, "usd": x.usd},
                    "leg2": {"ts": y.ts, "from": y.from, "sym": y.sym, "usd": y.usd}, "usd": lo}));
            }
        }
    }

    // ФОРМА 3: ТРЕТЯ СТОРОНА — актив пішов до B, а оплата прийшла продавцю від ІНШОЇ адреси.
    // Так часто розраховуються десеки: приймальний гаманець і платіжний різні.
    let mut third = 0usize;
    for ((a, b), v) in &legs {
        for x in v {
            if is_money(&x.sym) { continue; }                       // нога активу, не оплати
            let seller = &x.from;
            let buyer = if seller == a { b } else { a };
            let Some(inb) = inbox.get(seller) else { continue };
            for y in inb {
                if !is_money(&y.sym) || &y.from == seller || &y.from == buyer { continue; }
                if is_wrapper(&x.sym) { continue; }
                // платник мусить мати ПРЯМИЙ звʼязок із покупцем — інакше це просто будь-який
                // вхідний платіж у тому ж вікні, а таких у великій базі десятки тисяч
                let linked = { let k = if y.from < *buyer { (y.from.clone(), buyer.clone()) } else { (buyer.clone(), y.from.clone()) };
                               legs.contains_key(&k) };
                if !linked { continue; }
                let dt = if y.ts > x.ts { y.ts - x.ts } else { x.ts - y.ts };
                if dt > win { continue; }
                let (hi, lo) = if x.usd > y.usd { (x.usd, y.usd) } else { (y.usd, x.usd) };
                if hi <= 0.0 || (hi - lo) / hi > tol { continue; }
                third += 1;
                deals.push(json!({"a": seller, "b": buyer, "class": "КАНДИДАТ", "sameTx": false,
                    "form": "третя сторона", "payer": y.from, "money": true,
                    "gapMin": dt / 60, "diffPct": (hi - lo) / hi * 100.0,
                    "leg1": {"ts": x.ts, "from": x.from, "sym": x.sym, "usd": x.usd},
                    "leg2": {"ts": y.ts, "from": y.from, "sym": y.sym, "usd": y.usd}, "usd": lo}));
            }
        }
    }
    eprintln!("[otcfind] форми: атомарних знайдено у транзакціях · через третю сторону {third}");

    deals.sort_by(|p, q| q["usd"].as_f64().unwrap_or(0.0).partial_cmp(&p["usd"].as_f64().unwrap_or(0.0)).unwrap());

    let proofs = deals.iter().filter(|d| d["class"] == "ДОКАЗ").count();
    // скільки РІЗНИХ контрагентів має кожна адреса — підпис OTC-служби
    let mut partners: HashMap<String, std::collections::HashSet<String>> = HashMap::new();
    for d in &deals {
        let (a, b) = (d["a"].as_str().unwrap_or("").to_string(), d["b"].as_str().unwrap_or("").to_string());
        partners.entry(a.clone()).or_default().insert(b.clone());
        partners.entry(b).or_default().insert(a);
    }
    let mut pv: Vec<(&String, usize)> = partners.iter().map(|(a, s)| (a, s.len())).collect();
    pv.sort_by(|x, y| y.1.cmp(&x.1));

    eprintln!("[otcfind] ===== OTC-УГОДИ =====");
    eprintln!("[otcfind] знайдено {} (доказів {}, кандидатів {})", deals.len(), proofs, deals.len() - proofs);
    for d in deals.iter().take(12) {
        eprintln!("   {:<9} ${:>12.0} · {} ↔ {} · {} за {} хв, розбіжність {:.1}%",
            d["class"].as_str().unwrap_or(""), d["usd"].as_f64().unwrap_or(0.0),
            d["leg1"]["sym"].as_str().unwrap_or(""), d["leg2"]["sym"].as_str().unwrap_or(""),
            if d["sameTx"] == json!(true) { "одна tx" } else { "дві tx" },
            d["gapMin"].as_u64().unwrap_or(0), d["diffPct"].as_f64().unwrap_or(0.0));
    }
    eprintln!("[otcfind] --- адреси з найбільшою к-стю різних контрагентів (підпис служби) ---");
    for (a, n) in pv.iter().take(8) { eprintln!("   {} контрагентів ← {}", n, a); }
    let verdict = if pv.first().map(|x| x.1).unwrap_or(0) >= 10 {
        "є адреси з десятками різних контрагентів → підпис OTC-СЛУЖБИ, а не замкненого кола"
    } else if proofs > 0 { "угоди є, але коло контрагентів вузьке → радше внутрішні розрахунки" }
    else { "доведених OTC-угод не знайдено" };
    eprintln!("[otcfind] ВЕРДИКТ: {verdict}");
    std::fs::write(format!("{bf}/otc-{chain}.json"), serde_json::to_string_pretty(&json!({
        "chain": chain, "minUsd": min_usd, "windowHours": win / 3600, "tolerance": tol,
        "deals": deals.len(), "proofs": proofs, "verdict": verdict,
        "topPartners": pv.iter().take(30).map(|(a, n)| json!({"addr": a, "partners": n})).collect::<Vec<Value>>(),
        "list": deals.iter().take(500).collect::<Vec<&Value>>()})).unwrap()).ok();
    eprintln!("[otcfind] збережено otc-{chain}.json");
}
