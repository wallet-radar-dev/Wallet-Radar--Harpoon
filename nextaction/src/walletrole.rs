//! walletrole — ФУНКЦІЯ кожного гаманця мережі з реального руху грошей.
//!
//! Навіщо: тіри rglead рахувались на токенних входах і покрили лише 222 живі гаманці з 6283.
//! Тепер у нас є ETH-ребра З СУМАМИ по всій мережі (eth-edges2) + реальні баланси + L2 —
//! цього досить, щоб дати роль тисячам, а не сотням, і будувати сигнали не по 11 адресах.
//!
//! Ролі (пріоритет зверху вниз, детерміновано):
//!   КАЗНА/РОЗДАВАЧ/КОЛЕКТОР/РЕЛЕ — відомий кістяк
//!   ЦИРКУЛЯТОР — і отримує від роздавача, і шле у казну/колектори (замикає петлю)
//!   ОТРИМУВАЧ   — отримує від роздавача (будь-якого покоління)
//!   ФАНДЕР      — шле у казну/колектори
//!   ПРОДАВЕЦЬ   — ≥CEX_SHARE відтоку йде на біржі
//!   ТРАНЗИТ     — вихід ≥0.9 входу і залишок мізерний (гроші проходять наскрізь)
//!   ХОЛДЕР      — баланс ≥HOLD_ETH і мало виходу
//!   СПЛЯЧИЙ     — остання активність давніша за STALE днів
//!   ДРІБНИЙ     — решта
//! Плюс поля: hop, баланс, обіг, к-сть tx, на яких L2 активний, чи є токенні дані.
//! Вихід: dataset/backfill/wallet-roles.json + зведення. Args: ROOT. Env: CEX_SHARE(0.5) HOLD_ETH(5) STALE(180)
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

/// Ролі мережі переїхали у базу (`wallet_tags`, теги `walletrole:*`).
fn tagged(tag: &str) -> Vec<String> {
    let c = wr_store::open().expect("walletrole: без сховища немає ролей мережі");
    let mut q = c
        .prepare("SELECT address FROM wallet_tags WHERE tag=?1")
        .expect("запит тегу");
    let v: Vec<String> = q
        .query_map([tag], |r| r.get(0))
        .expect("читання тегу")
        .filter_map(|r| r.ok())
        .collect();
    assert!(!v.is_empty(), "walletrole: тег {tag} порожній — ролі не розставити");
    v
}
use std::fs::File;
use std::io::{BufRead, BufReader};






fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let cex_share: f64 = std::env::var("CEX_SHARE").ok().and_then(|s| s.parse().ok()).unwrap_or(0.5);
    let hold_eth: f64 = std::env::var("HOLD_ETH").ok().and_then(|s| s.parse().ok()).unwrap_or(5.0);
    let stale: u64 = std::env::var("STALE").ok().and_then(|s| s.parse().ok()).unwrap_or(180) * 86400;
    let bf = format!("{root}/dataset/backfill");
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();

    // членство + хопи
    let memb: Value = serde_json::from_reader(BufReader::new(File::open(format!("{bf}/group-membership.json")).expect("membership"))).unwrap();
    let mut grp: HashMap<String, (String, i64)> = HashMap::new();
    for (w, x) in memb["wallets"].as_object().cloned().unwrap_or_default() {
        grp.insert(w.to_lowercase(), (x["group"].as_str().unwrap_or("?").to_string(), x["hop"].as_i64().unwrap_or(-1)));
    }
    // баланси
    let bal: HashMap<String, f64> = serde_json::from_reader::<_, Value>(BufReader::new(File::open(format!("{bf}/eth-balances.json")).expect("balances"))).ok()
        .and_then(|v| v["wallets"].as_object().map(|m| m.iter().filter_map(|(k, x)| x["eth"].as_f64().map(|e| (k.to_lowercase(), e))).collect())).unwrap_or_default();
    // біржі
    let lbl: serde_json::Map<String, Value> = serde_json::from_reader(BufReader::new(File::open(format!("{root}/dataset/labels-cache.json")).unwrap())).unwrap_or_default();
    let mut kind: HashMap<String, String> = HashMap::new();
    if let Ok(v) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(File::open(format!("{bf}/wallet-kind.json")).unwrap())) {
        for (a, k) in &v { kind.insert(a.to_lowercase(), k.as_str().unwrap_or("").to_string()); }
    }
    let is_cex = |a: &str| matches!(kind.get(a).map(|s| s.as_str()), Some("exchange") | Some("otc"))
        || lbl.get(a).and_then(|x| x["name"].as_str()).map(|n| { let l = n.to_lowercase();
            ["binance","coinbase","okx","gate","kraken","bybit","kucoin","bitget","huobi","mexc","bitfinex","upbit","bithumb","paribu","btcturk","gopax","crypto.com","exchange","hot wallet","deposit"].iter().any(|c| l.contains(c)) }).unwrap_or(false);
    // хто має токенні дані
    let mut hasrows: HashSet<String> = HashSet::new();
    if let Ok(f) = File::open(format!("{bf}/transfers.csv")) {
        for line in BufReader::new(f).lines().map_while(Result::ok) {
            if let Some(w) = line.split(',').next() { if w.len() == 42 { hasrows.insert(w.to_lowercase()); } }
        }
    }
    // активність на L2
    let mut l2: HashMap<String, Vec<&str>> = HashMap::new();
    for (file, name) in [("eth-edges2-42161.jsonl", "arb"), ("eth-edges2-8453.jsonl", "base"), ("eth-edges2-324.jsonl", "zksync")] {
        if let Ok(f) = File::open(format!("{bf}/{file}")) {
            for line in BufReader::new(f).lines().map_while(Result::ok) {
                if let Ok(v) = serde_json::from_str::<Value>(&line) { if let Some(w) = v["w"].as_str() { l2.entry(w.to_lowercase()).or_default().push(name); } }
            }
        }
    }
    // класи потоків (flowclass): реальний продаж vs ліквідність vs міст — «вихід на контракт»
    // сам по собі нічого не означає, з $93.4М такого виходу продажем виявились лише $9.06М
    let mut flow: HashMap<String, (f64, f64, f64)> = HashMap::new();   // swap, lp, bridge
    if let Ok(v) = serde_json::from_reader::<_, Value>(BufReader::new(File::open(format!("{bf}/flow-classes.json")).unwrap())) {
        for (w, x) in v["wallets_map"].as_object().cloned().unwrap_or_default() {
            flow.insert(w.to_lowercase(), (x["swapUsd"].as_f64().unwrap_or(0.0), x["lpUsd"].as_f64().unwrap_or(0.0), x["bridgeUsd"].as_f64().unwrap_or(0.0)));
        }
    }
    let liq_min: f64 = std::env::var("LIQ_MIN_USD").ok().and_then(|s| s.parse().ok()).unwrap_or(50_000.0);
    let bridge_min: f64 = std::env::var("BRIDGE_MIN_USD").ok().and_then(|s| s.parse().ok()).unwrap_or(10_000.0);

    // тіри rglead (де є)
    let mut tier: HashMap<String, String> = HashMap::new();
    if let Ok(v) = serde_json::from_reader::<_, Value>(BufReader::new(File::open(format!("{bf}/rglead.json")).unwrap())) {
        for w in v["wallets"].as_array().cloned().unwrap_or_default() {
            if let Some(a) = w["w"].as_str() { tier.insert(a.to_lowercase(), w["tier"].as_str().unwrap_or("").to_string()); }
        }
    }

    let treasury = tagged("walletrole:treasury");
    let dispensers = tagged("walletrole:dispensers");
    let collectors = tagged("walletrole:collectors");
    let relays = tagged("walletrole:relays");
    let skel: HashMap<&str, &str> = treasury.iter().map(|a| (a.as_str(), "КАЗНА"))
        .chain(dispensers.iter().map(|a| (a.as_str(), "РОЗДАВАЧ")))
        .chain(collectors.iter().map(|a| (a.as_str(), "КОЛЕКТОР")))
        .chain(relays.iter().map(|a| (a.as_str(), "РЕЛЕ"))).collect();
    let disp: HashSet<&str> = dispensers.iter().map(String::as_str).collect();
    let sink: HashSet<&str> =
        treasury.iter().chain(collectors.iter()).map(String::as_str).collect();

    // основний прохід по ETH-ребрах
    let mut out: Vec<Value> = Vec::new();
    let mut counts: HashMap<String, (usize, f64)> = HashMap::new();
    for line in BufReader::new(File::open(format!("{bf}/eth-edges2.jsonl")).expect("eth-edges2")).lines().map_while(Result::ok) {
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        let w = v["w"].as_str().unwrap_or("").to_lowercase();
        let Some((g, hop)) = grp.get(&w).cloned() else { continue };
        if g == "INFRA" || g == "NODATA" { continue; }
        let (tin, tout) = (v["in"].as_f64().unwrap_or(0.0), v["out"].as_f64().unwrap_or(0.0));
        let (ntx, last) = (v["n"].as_u64().unwrap_or(0), v["last"].as_u64().unwrap_or(0));
        let b = bal.get(&w).copied().unwrap_or(0.0);
        let (mut from_disp, mut to_sink, mut cex_out) = (false, false, 0.0f64);
        for c in v["cps"].as_array().cloned().unwrap_or_default() {
            let a = c["a"].as_str().unwrap_or("");
            let (i, o) = (c["i"].as_f64().unwrap_or(0.0), c["o"].as_f64().unwrap_or(0.0));
            if disp.contains(a) && i > 0.0 { from_disp = true; }
            if sink.contains(a) && o > 0.0 { to_sink = true; }
            if is_cex(a) { cex_out += o; }
        }
        let cex_frac = if tout > 0.0 { cex_out / tout } else { 0.0 };
        let role = if let Some(r) = skel.get(w.as_str()) { r.to_string() }
            else if from_disp && to_sink { "ЦИРКУЛЯТОР".into() }
            else if from_disp { "ОТРИМУВАЧ".into() }
            else if to_sink { "ФАНДЕР".into() }
            else if cex_frac >= cex_share && tout > 0.1 { "ПРОДАВЕЦЬ".into() }
            else if flow.get(&w).map(|f| f.1 >= liq_min && f.1 > f.0).unwrap_or(false) { "ЛІКВІДНІСТЬ".into() }
            else if flow.get(&w).map(|f| f.2 >= bridge_min).unwrap_or(false) { "МІСТ".into() }
            else if tin > 0.1 && tout >= 0.9 * tin && b < 0.05 * tin.max(0.001) { "ТРАНЗИТ".into() }
            else if b >= hold_eth && tout < 0.5 * tin { "ХОЛДЕР".into() }
            // НЕ «сплячий»: тут остання активність рахується лише по нативному ETH, а гаманець
            // цілком може рухати токени. Тому чесна назва — «без ETH-руху».
            else if last > 0 && now.saturating_sub(last) > stale { "БЕЗ-ETH-РУХУ".into() }
            else { "ДРІБНИЙ".into() };
        let e = counts.entry(role.clone()).or_insert((0, 0.0)); e.0 += 1; e.1 += b;
        out.push(json!({"w": w, "group": g, "hop": if hop < 0 { Value::Null } else { json!(hop) }, "role": role,
            "balEth": b, "inEth": tin, "outEth": tout, "tx": ntx, "cexShare": cex_frac,
            "l2": l2.get(&w).cloned().unwrap_or_default(), "tokenData": hasrows.contains(&w),
            "leadTier": tier.get(&w).cloned().unwrap_or_default(), "lastTs": last}));
    }
    out.sort_by(|a, b| b["balEth"].as_f64().unwrap_or(0.0).partial_cmp(&a["balEth"].as_f64().unwrap_or(0.0)).unwrap());

    let mut cv: Vec<(&String, &(usize, f64))> = counts.iter().collect();
    cv.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));
    eprintln!("[walletrole] ===== ФУНКЦІЯ ГАМАНЦІВ МЕРЕЖІ =====");
    eprintln!("{:<14} {:>7} {:>16}", "роль", "гаманців", "ETH на них");
    for (r, (n, b)) in &cv { eprintln!("{:<14} {:>7} {:>16.1}", r, n, b); }
    eprintln!("[walletrole] розписано {} гаманців (було з тіром лише {})",
        out.len(), out.iter().filter(|x| !x["leadTier"].as_str().unwrap_or("").is_empty()).count());
    let l2n = out.iter().filter(|x| !x["l2"].as_array().map(|a| a.is_empty()).unwrap_or(true)).count();
    eprintln!("[walletrole] з них активні на L2: {l2n} · мають токенні дані: {}", out.iter().filter(|x| x["tokenData"] == json!(true)).count());
    std::fs::write(format!("{bf}/wallet-roles.json"), serde_json::to_string(&json!({
        "generated": now, "wallets": out,
        "summary": cv.iter().map(|(r, (n, b))| json!({"role": r, "n": n, "eth": b})).collect::<Vec<Value>>()})).unwrap()).ok();
    eprintln!("[walletrole] збережено wallet-roles.json");
}
