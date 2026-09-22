//! cexcrawl — РЕД-ТІМ ТЕСТ ради #4: чи ядрові гаманці КОЛИ-НЕБУДЬ ходили на великі CEX НАПРЯМУ
//! (до маршрутизації через хаб 0x6fc48de8). Якщо прямих депозитів майже нема — версія «спільна
//! біржова інфраструктура» слабшає, лишається координація через реле.
//! Побічно дає ДАНІ для сигналу (#1) і лід-лагу (#3): усі події ядро↔біржа з часом і сумою в USD,
//! + останню активність кожного ядрового (живий/сплячий).
//!
//! Etherscan V2 chainid=1 (txlist + tokentx), ротація BACKFILL_ETHERSCAN_KEYS, резюмований.
//! Виходи: dataset/backfill/core-cex-events.jsonl {w,ts,dir,ex,exAddr,asset,amt,usd}
//!         dataset/backfill/core-cex-done.txt        (резюм: пройдені гаманці)
//!         dataset/backfill/core-activity.json       {addr:{last,first,txs}}
//!         dataset/backfill/core-cex-summary.json    (вердикт ред-тіму)
//! Args: ROOT. Env: COV(5) CONC(8) MAJOR_ONLY(0)
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::Semaphore;

/// біржові родини; true = вимагати, щоб мітка ПОЧИНАЛАСЬ із терміну (коротке/двозначне слово)
const FAMS: [(&str, bool); 22] = [
    ("binance", false), ("kraken", false), ("okx", false), ("gopax", false), ("coinbase", false),
    ("bybit", false), ("kucoin", false), ("huobi", false), ("bitfinex", false), ("upbit", false),
    ("bithumb", false), ("mexc", false), ("bitget", false), ("paribu", false), ("btcturk", false),
    ("bingx", false), ("crypto.com", false), ("bitstamp", false), ("gemini", false),
    ("poloniex", false), ("gate", true), ("htx", true),
];
/// «великі» біржі з ради (кешаут-вихід) + Paribu (вхід)
const MAJOR: [&str; 8] = ["kraken", "binance", "okx", "gopax", "coinbase", "bybit", "paribu", "htx"];

fn family(name: &str) -> Option<&'static str> {
    let n = name.to_lowercase();
    if n.contains("bridge") || n.contains("gateway") || n.contains("uniswap") || n.contains("pool") { return None; }
    for (t, prefix) in FAMS {
        let hit = if prefix { n.starts_with(t) } else { n.contains(t) };
        if hit { return Some(t); }
    }
    None
}

fn es_url(keys: &[String], kc: &AtomicUsize, params: &str) -> String {
    let key = &keys[kc.fetch_add(1, Ordering::Relaxed) % keys.len()];
    format!("https://api.etherscan.io/v2/api?chainid=1&{params}&apikey={key}")
}
/// Скільки записів реально віддає Etherscan за запит (offset=10000 однаково повертає 1000).
const PAGE: usize = 1000;
/// Запобіжник від нескінченної пагінації.
const MAX_WINDOWS: usize = 400;

/// Уся історія гаманця по одному `action`, партіями по `PAGE` зі зсувом startblock.
///
/// Раніше робився ОДИН запит із `offset=10000`, і в гаманця з понад 1000 переказів бралась лише
/// найдавніша тисяча — тобто свіжі депозити на біржу просто не існували для аналізу. Гірше:
/// ознака обрізання перевірялась як `len() >= 10000` і не спрацьовувала НІКОЛИ, бо стеля 1000.
/// Дані виглядали повними. Повертає (записи, чи вперлись у стелю вікон).
async fn es_all(client: &reqwest::Client, keys: &[String], kc: &AtomicUsize, action: &str, w: &str)
    -> Option<(Vec<Value>, bool)>
{
    let mut all: Vec<Value> = Vec::new();
    let mut from_block: u64 = 0;
    for i in 0..MAX_WINDOWS {
        let part = es(client, keys, kc, &format!(
            "module=account&action={action}&address={w}&startblock={from_block}&endblock=99999999&page=1&offset={PAGE}&sort=asc")).await?;
        let n = part.len();
        let mut max_blk = from_block;
        for t in &part {
            if let Some(b) = t["blockNumber"].as_str().and_then(|s| s.parse::<u64>().ok()) {
                if b > max_blk { max_blk = b; }
            }
        }
        all.extend(part);
        if n < PAGE || max_blk <= from_block {
            return Some((all, false));
        }
        from_block = max_blk;
        if i + 1 == MAX_WINDOWS { return Some((all, true)); }
    }
    Some((all, true))
}

/// `Some(vec)` — валідна відповідь (порожній = «No transactions found»), `None` — стійкий ліміт.
///
/// Etherscan під лімітом віддає HTTP **200** з тілом
/// `{"status":"0","message":"NOTOK","result":"Max rate limit reached"}` — `result` там РЯДОК,
/// тож `as_array()` мовчки не спрацьовував, спроби вигорали, поверталось `Vec::new()`, і
/// викликач писав гаманець у `core-cex-done.txt` з нулем подій. Для монітора в cron це
/// найгірший варіант: «біржових депозитів нема» від залімічених гаманців — це хибний сигнал.
async fn es(client: &reqwest::Client, keys: &[String], kc: &AtomicUsize, params: &str) -> Option<Vec<Value>> {
    for attempt in 0..7u64 {
        if let Ok(r) = client.get(es_url(keys, kc, params)).send().await {
            let st = r.status().as_u16();
            if st != 429 && st < 500 {
                if let Ok(v) = r.json::<Value>().await {
                    let msg = v["message"].as_str().unwrap_or("");
                    let res = v["result"].as_str().unwrap_or("");
                    let limited = msg == "NOTOK"
                        || res.contains("rate limit")
                        || res.contains("Max calls")
                        || res.contains("Too many");
                    if !limited {
                        if let Some(a) = v["result"].as_array() { return Some(a.clone()); }
                        if msg == "No transactions found" { return Some(Vec::new()); }
                    }
                }
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(400 * (1u64 << attempt.min(6)))).await;
    }
    None
}

fn envv(root: &str, k: &str) -> String {
    fs::read_to_string(format!("{root}/.env")).unwrap_or_default().lines()
        .find(|l| l.starts_with(&format!("{k}="))).map(|l| l.splitn(2, '=').nth(1).unwrap_or("").trim().to_string()).unwrap_or_default()
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let cov: u64 = std::env::var("COV").ok().and_then(|s| s.parse().ok()).unwrap_or(5);
    let conc: usize = std::env::var("CONC").ok().and_then(|s| s.parse().ok()).unwrap_or(8);
    let bf = format!("{root}/dataset/backfill");
    let keys: Vec<String> = envv(&root, "BACKFILL_ETHERSCAN_KEYS").split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    assert!(!keys.is_empty(), "нема BACKFILL_ETHERSCAN_KEYS у .env");

    // --- ядро: rglead coverage>=COV, лише EOA (контракт-пули не є «ядром») ---
    let lead: Value = serde_json::from_reader(BufReader::new(File::open(format!("{bf}/rglead.json")).expect("rglead.json"))).expect("rglead parse");
    let mut cores: Vec<String> = Vec::new();
    for w in lead["wallets"].as_array().cloned().unwrap_or_default() {
        if w["coverage"].as_u64().unwrap_or(0) >= cov && w["kind"].as_str() != Some("contract") {
            if let Some(a) = w["w"].as_str() { cores.push(a.to_lowercase()); }
        }
    }
    cores.sort(); cores.dedup();

    // --- біржові адреси з labels-cache ---
    let lbl: serde_json::Map<String, Value> = serde_json::from_reader(BufReader::new(File::open(format!("{root}/dataset/labels-cache.json")).expect("labels"))).unwrap_or_default();
    let mut ex: HashMap<String, (&'static str, String)> = HashMap::new(); // addr -> (родина, повна мітка)
    for (a, x) in &lbl {
        let name = x["name"].as_str().unwrap_or("");
        if let Some(f) = family(name) { ex.insert(a.to_lowercase(), (f, name.to_string())); }
    }
    // --- ціни: ринкові (DefiLlama, бін tokprices) з відкотом на ончейн-своп-ціну, якщо вона осудна ---
    let tp: Value = serde_json::from_reader(BufReader::new(File::open(format!("{bf}/token-prices.json")).expect("token-prices.json — спершу прогнати tokprices"))).expect("token-prices parse");
    let mut px: HashMap<String, f64> = tp["tokens"].as_object().map(|m| m.iter().filter_map(|(k, v)| v["usd"].as_f64().map(|u| (k.to_lowercase(), u))).collect()).unwrap_or_default();
    let real_n = px.len();
    let onchain: serde_json::Map<String, Value> = serde_json::from_reader(BufReader::new(File::open(format!("{bf}/onchain-prices.json")).expect("prices"))).unwrap_or_default();
    for (k, v) in &onchain {
        let k = k.to_lowercase();
        if px.contains_key(&k) { continue; }
        // ончейн-ціни містять артефакти (трильйони, COMP ×2350) — беремо лише правдоподібні
        if let Some(u) = v["usd"].as_f64() { if u > 0.0 && u < 10_000.0 { px.insert(k, u); } }
    }
    let eth_usd: f64 = tp["eth"].as_f64().filter(|e| *e > 0.0).unwrap_or_else(|| {
        serde_json::from_reader::<_, Value>(BufReader::new(File::open(format!("{bf}/capital-summary.json")).unwrap())).ok()
            .and_then(|v| v["eth_price"].as_f64()).unwrap_or(1860.0)
    });
    eprintln!("[cexcrawl] цін: {real_n} ринкових + {} відкотних (осудних ончейн) · ETH ${eth_usd:.2}", px.len() - real_n);

    // --- резюм ---
    let outp = format!("{bf}/core-cex-events.jsonl");
    let donep = format!("{bf}/core-cex-done.txt");
    let mut done: HashSet<String> = HashSet::new();
    if let Ok(f) = File::open(&donep) { for l in BufReader::new(f).lines().map_while(Result::ok) { done.insert(l.trim().to_lowercase()); } }
    let todo: Vec<String> = cores.iter().filter(|c| !done.contains(*c)).cloned().collect();
    eprintln!("[cexcrawl] ядрових (coverage>={cov}, eoa): {} · біржових адрес у мітках: {} · вже зроблено: {} · до краулу: {} (ключів {}, conc {})",
        cores.len(), ex.len(), done.len(), todo.len(), keys.len(), conc);

    let keys = Arc::new(keys);
    let kc = Arc::new(AtomicUsize::new(0));
    let ex = Arc::new(ex);
    let px = Arc::new(px);
    let client = Arc::new(reqwest::Client::builder().user_agent("wallet-radar-cexcrawl/0.1").pool_max_idle_per_host(conc).timeout(std::time::Duration::from_secs(40)).build().unwrap());
    let sem = Arc::new(Semaphore::new(conc));
    let out = Arc::new(Mutex::new(OpenOptions::new().create(true).append(true).open(&outp).unwrap()));
    let donef = Arc::new(Mutex::new(OpenOptions::new().create(true).append(true).open(&donep).unwrap()));
    let act: Arc<Mutex<HashMap<String, Value>>> = Arc::new(Mutex::new(
        fs::read_to_string(format!("{bf}/core-activity.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()));
    let progress = Arc::new(AtomicUsize::new(0));
    let total = todo.len();

    let mut handles = Vec::new();
    for w in todo {
        let (keys, kc, client, sem, out, donef, ex, px, act, progress) =
            (keys.clone(), kc.clone(), client.clone(), sem.clone(), out.clone(), donef.clone(), ex.clone(), px.clone(), act.clone(), progress.clone());
        handles.push(tokio::spawn(async move {
            let _p = sem.acquire().await.unwrap();
            let mut evs: Vec<Value> = Vec::new();
            let (mut first, mut last, mut ntx) = (u64::MAX, 0u64, 0usize);

            // 1) нативний ETH
            let Some((txs, eth_trunc)) = es_all(&client, &keys, &kc, "txlist", &w).await else {
                // ліміт не здолано — не пишемо ні подій, ні активності, ні done: гаманець
                // лишається в черзі. Інакше він назавжди зафіксувався б як «без депозитів».
                let n = progress.fetch_add(1, Ordering::Relaxed) + 1;
                if n % 25 == 0 || n == total { eprintln!("[cexcrawl] {n}/{total}"); }
                return;
            };
            for t in &txs {
                if t["isError"].as_str() == Some("1") { continue; }
                let ts: u64 = t["timeStamp"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0);
                if ts > 0 { ntx += 1; if ts < first { first = ts } if ts > last { last = ts } }
                let val: f64 = t["value"].as_str().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0) / 1e18;
                if val <= 0.0 { continue; }
                let frm = t["from"].as_str().unwrap_or("").to_lowercase();
                let to = t["to"].as_str().unwrap_or("").to_lowercase();
                let is_out = frm == w;
                let other = if is_out { &to } else { &frm };
                if let Some((fam, label)) = ex.get(other) {
                    evs.push(json!({"w": w, "ts": ts, "dir": if is_out {"out"} else {"in"}, "ex": fam, "exAddr": other,
                                    "exLabel": label, "asset": "ETH", "amt": val, "usd": val * eth_usd}));
                }
            }
            // 2) ERC-20
            let Some((tks, tok_trunc)) = es_all(&client, &keys, &kc, "tokentx", &w).await else {
                let n = progress.fetch_add(1, Ordering::Relaxed) + 1;
                if n % 25 == 0 || n == total { eprintln!("[cexcrawl] {n}/{total}"); }
                return;
            };
            for t in &tks {
                let ts: u64 = t["timeStamp"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0);
                if ts > 0 { ntx += 1; if ts < first { first = ts } if ts > last { last = ts } }
                let frm = t["from"].as_str().unwrap_or("").to_lowercase();
                let to = t["to"].as_str().unwrap_or("").to_lowercase();
                let is_out = frm == w;
                let other = if is_out { &to } else { &frm };
                let Some((fam, label)) = ex.get(other) else { continue };
                let dec: i32 = t["tokenDecimal"].as_str().and_then(|s| s.parse().ok()).unwrap_or(18);
                let raw: f64 = t["value"].as_str().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
                let amt = raw / 10f64.powi(dec);
                if amt <= 0.0 { continue; }
                let contract = t["contractAddress"].as_str().unwrap_or("").to_lowercase();
                let usd = px.get(&contract).map(|p| p * amt).unwrap_or(0.0);
                evs.push(json!({"w": w, "ts": ts, "dir": if is_out {"out"} else {"in"}, "ex": fam, "exAddr": other,
                                "exLabel": label, "asset": t["tokenSymbol"].as_str().unwrap_or("?"), "tok": contract, "amt": amt, "usd": usd}));
            }

            {
                let mut o = out.lock().unwrap();
                for e in &evs { let _ = writeln!(o, "{}", e); }
            }
            act.lock().unwrap().insert(w.clone(), json!({"first": if first == u64::MAX {0} else {first}, "last": last, "txs": ntx,
                                                          "cexEvents": evs.len(), "truncated": eth_trunc || tok_trunc}));
            { let mut d = donef.lock().unwrap(); let _ = writeln!(d, "{w}"); }
            let n = progress.fetch_add(1, Ordering::Relaxed) + 1;
            if n % 25 == 0 || n == total { eprintln!("[cexcrawl] {n}/{total}"); }
        }));
    }
    for h in handles { let _ = h.await; }

    // --- зведення + вердикт ---
    let acts = act.lock().unwrap().clone();
    let _ = fs::write(format!("{bf}/core-activity.json"), serde_json::to_string_pretty(&json!(acts)).unwrap());

    let sig_usd: f64 = std::env::var("SIG_USD").ok().and_then(|s| s.parse().ok()).unwrap_or(10_000.0);
    let mut out_major: HashSet<String> = HashSet::new();   // депонував напряму на ВЕЛИКУ біржу
    let mut out_major_usd: HashMap<String, f64> = HashMap::new(); // і на скільки $
    let mut out_any: HashSet<String> = HashSet::new();
    let mut in_any: HashSet<String> = HashSet::new();
    let mut per_ex: HashMap<String, (f64, f64, usize, usize)> = HashMap::new(); // родина -> (usd_out, usd_in, n_out, n_in)
    let (mut ts_min, mut ts_max) = (u64::MAX, 0u64);
    let mut n_ev = 0usize;
    if let Ok(f) = File::open(&outp) {
        for line in BufReader::new(f).lines().map_while(Result::ok) {
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            n_ev += 1;
            let w = v["w"].as_str().unwrap_or("").to_string();
            let fam = v["ex"].as_str().unwrap_or("?").to_string();
            let usd = v["usd"].as_f64().unwrap_or(0.0);
            let ts = v["ts"].as_u64().unwrap_or(0);
            if ts > 0 { if ts < ts_min { ts_min = ts } if ts > ts_max { ts_max = ts } }
            let e = per_ex.entry(fam.clone()).or_insert((0.0, 0.0, 0, 0));
            if v["dir"] == "out" {
                out_any.insert(w.clone()); e.0 += usd; e.2 += 1;
                if MAJOR.contains(&fam.as_str()) { *out_major_usd.entry(w.clone()).or_insert(0.0) += usd; out_major.insert(w); }
            } else { in_any.insert(w); e.1 += usd; e.3 += 1; }
        }
    }
    let crawled: HashSet<String> = acts.keys().cloned().collect();
    let touched: HashSet<String> = out_any.union(&in_any).cloned().collect();
    let never = crawled.len().saturating_sub(touched.len());
    let in_only = in_any.difference(&out_any).count();
    let pct = |n: usize| if crawled.is_empty() { 0.0 } else { 100.0 * n as f64 / crawled.len() as f64 };

    let sig: usize = out_major_usd.values().filter(|u| **u >= sig_usd).count();
    let verdict = if pct(sig) < 15.0 {
        "ядро майже НЕ депонує на великі CEX напряму → кешаут іде ЧЕРЕЗ посередників; версія «спільна біржова інфраструктура» СЛАБНЕ"
    } else if pct(sig) < 50.0 {
        "частина ядра депонує напряму, частина — ні: змішана картина, версія спільної інфраструктури НЕ відкидається"
    } else {
        "більшість ядра депонує на великі CEX НАПРЯМУ → маршрутизація через хаб не є обов'язковою, тезу обфускації треба переглянути"
    };

    let mut exv: Vec<Value> = per_ex.iter().map(|(f, (uo, ui, no, ni))| json!({"ex": f, "usdOut": uo, "usdIn": ui, "nOut": no, "nIn": ni})).collect();
    exv.sort_by(|a, b| b["usdOut"].as_f64().unwrap_or(0.0).partial_cmp(&a["usdOut"].as_f64().unwrap_or(0.0)).unwrap());

    let summary = json!({
        "cores": crawled.len(), "coverageMin": cov, "events": n_ev,
        "coresDepositingMajorCex": out_major.len(), "pctDepositingMajorCex": pct(out_major.len()),
        "coresDepositingMajorSignificant": sig, "pctSignificant": pct(sig), "sigUsdThreshold": sig_usd,
        "coresDepositingAnyCex": out_any.len(), "pctDepositingAnyCex": pct(out_any.len()),
        "coresReceivingOnly": in_only, "coresNoCexAtAll": never,
        "tsMin": if ts_min == u64::MAX {0} else {ts_min}, "tsMax": ts_max,
        "byExchange": exv, "verdict": verdict,
        "coresDepositingList": out_major.iter().cloned().collect::<Vec<String>>(),
    });
    let _ = fs::write(format!("{bf}/core-cex-summary.json"), serde_json::to_string_pretty(&summary).unwrap());

    eprintln!("[cexcrawl] ===== РЕД-ТІМ: прямі зв'язки ядро↔CEX =====");
    eprintln!("[cexcrawl] ядрових пройдено: {} · подій ядро↔біржа: {}", crawled.len(), n_ev);
    eprintln!("[cexcrawl] депонують НАПРЯМУ на ВЕЛИКУ біржу: {} ({:.1}%) · з них ЗНАЧУЩО (≥${:.0}): {} ({:.1}%)",
        out_major.len(), pct(out_major.len()), sig_usd, sig, pct(sig));
    eprintln!("[cexcrawl] депонують на будь-яку біржу: {} ({:.1}%)", out_any.len(), pct(out_any.len()));
    eprintln!("[cexcrawl] лише ОТРИМУЮТЬ з біржі (вивід): {} · зовсім без CEX: {}", in_only, never);
    for e in exv.iter().take(12) {
        eprintln!("     {:<12} депозити ${:>14.0} ({} шт) · виводи ${:>14.0} ({} шт)",
            e["ex"].as_str().unwrap_or("?"), e["usdOut"].as_f64().unwrap_or(0.0), e["nOut"].as_u64().unwrap_or(0),
            e["usdIn"].as_f64().unwrap_or(0.0), e["nIn"].as_u64().unwrap_or(0));
    }
    eprintln!("[cexcrawl] ВЕРДИКТ: {verdict}");
    eprintln!("[cexcrawl] збережено core-cex-events.jsonl / core-cex-summary.json / core-activity.json");
}
