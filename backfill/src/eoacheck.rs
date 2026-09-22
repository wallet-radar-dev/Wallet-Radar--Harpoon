//! eoacheck — чи є немічена EOA біржовою адресою насправді.
//!
//! Навіщо: 75.5% виходів групи ($262 млрд) припало на «звичайні гаманці». Але мітки є лише
//! для 29 772 адрес, тож серед немічених цілком можуть бути депозитні адреси бірж — і тоді
//! висновок «група переказує між своїми, а не торгує» хибний.
//!
//! Як розрізняємо (за формою потоку, а не за назвою):
//!   * ДЕПОЗИТНА адреса біржі — багато входів від РІЗНИХ, а вихід майже весь на ОДНУ
//!     адресу, і та адреса має біржову мітку. Класика: «воронка» з коефіцієнтом ~1.0.
//!   * ГАРЯЧИЙ ГАМАНЕЦЬ біржі — тисячі контрагентів з обох боків, величезний обіг.
//!   * СХОВИЩЕ — мало операцій, великі суми, вихід на кілька адрес.
//!   * Звичайний гаманець — нічого з переліченого.
//!
//! Вихід: dataset/backfill/eoacheck.json. Args: ROOT ADDR_FILE. Env: PAGES(1) OFFSET(400)
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::sync::atomic::{AtomicUsize, Ordering};

fn envn<T: std::str::FromStr>(k: &str, d: T) -> T {
    std::env::var(k).ok().and_then(|s| s.parse().ok()).unwrap_or(d)
}
fn envv(root: &str, k: &str) -> String {
    std::fs::read_to_string(format!("{root}/.env")).unwrap_or_default().lines()
        .find(|l| l.starts_with(&format!("{k}=")))
        .map(|l| l.splitn(2, '=').nth(1).unwrap_or("").trim().to_string()).unwrap_or_default()
}

const CEX_WORDS: [&str; 18] = ["binance", "coinbase", "okx", "kraken", "bybit", "kucoin",
    "gate", "bitfinex", "huobi", "mexc", "crypto.com", "bitget", "upbit", "bithumb",
    "gemini", "bitstamp", "hot wallet", "deposit"];

async fn es(c: &reqwest::Client, keys: &[String], kc: &AtomicUsize, params: &str) -> Option<Vec<Value>> {
    for attempt in 0..6u64 {
        let key = &keys[kc.fetch_add(1, Ordering::Relaxed) % keys.len()];
        let url = format!("https://api.etherscan.io/v2/api?chainid=1&{params}&apikey={key}");
        if let Ok(r) = c.get(&url).send().await {
            if let Ok(v) = r.json::<Value>().await {
                let msg = v["message"].as_str().unwrap_or("");
                let s = v["result"].as_str().unwrap_or("");
                let limited = msg == "NOTOK" || s.contains("rate limit")
                    || s.contains("Max calls") || s.contains("Too many");
                if !limited {
                    if let Some(a) = v["result"].as_array() { return Some(a.clone()) }
                    if msg == "No transactions found" { return Some(Vec::new()) }
                }
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(400 * (1u64 << attempt.min(5)))).await;
    }
    None
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let bf = format!("{root}/dataset/backfill");
    let afile = std::env::args().nth(2).expect("вкажи файл з адресами");
    let offset: u32 = envn("OFFSET", 400);

    let keys: Vec<String> = envv(&root, "BACKFILL_ETHERSCAN_KEYS")
        .split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    assert!(!keys.is_empty());

    let mut labels: HashMap<String, String> = HashMap::new();
    if let Ok(f) = File::open(format!("{bf}/labels-cache.json")) {
        if let Ok(m) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(f)) {
            for (a, v) in m {
                let mut s = v["name"].as_str().unwrap_or("").to_string();
                if let Some(arr) = v["labels"].as_array() {
                    for x in arr { if let Some(t) = x.as_str() { s.push(' '); s.push_str(t) } }
                }
                if !s.trim().is_empty() { labels.insert(a.to_lowercase(), s.to_lowercase()); }
            }
        }
    }
    let is_cex = |a: &str| labels.get(a).map(|l| CEX_WORDS.iter().any(|w| l.contains(w))).unwrap_or(false);

    let addrs: Vec<String> = BufReader::new(File::open(&afile).unwrap()).lines()
        .map_while(Result::ok).map(|l| l.trim().to_lowercase())
        .filter(|l| l.len() == 42).collect();
    eprintln!("[eoacheck] адрес: {} · міток: {}", addrs.len(), labels.len());

    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(40)).build().unwrap();
    let kc = AtomicUsize::new(0);
    let mut out: Vec<Value> = Vec::new();

    for (i, a) in addrs.iter().enumerate() {
        let txs = es(&client, &keys, &kc,
            &format!("module=account&action=txlist&address={a}&page=1&offset={offset}&sort=desc")).await
            .unwrap_or_default();
        let toks = es(&client, &keys, &kc,
            &format!("module=account&action=tokentx&address={a}&page=1&offset={offset}&sort=desc")).await
            .unwrap_or_default();

        let mut senders: HashSet<String> = HashSet::new();
        let mut recips: HashMap<String, u32> = HashMap::new();
        let mut n_in = 0u32;
        let mut n_out = 0u32;
        for t in txs.iter().chain(toks.iter()) {
            let f = t["from"].as_str().unwrap_or("").to_lowercase();
            let to = t["to"].as_str().unwrap_or("").to_lowercase();
            if to == *a { senders.insert(f); n_in += 1 }
            else if f == *a { *recips.entry(to).or_insert(0) += 1; n_out += 1 }
        }
        // куди йде основний вихід
        let mut rv: Vec<(&String, &u32)> = recips.iter().collect();
        rv.sort_by(|x, y| y.1.cmp(x.1));
        let (top_dst, top_cnt) = rv.first().map(|(d, c)| ((*d).clone(), **c)).unwrap_or_default();
        let funnel = if n_out > 0 { top_cnt as f64 / n_out as f64 } else { 0.0 };
        let top_is_cex = is_cex(&top_dst);

        let verdict = if top_is_cex && funnel > 0.6 && senders.len() > 5 {
            "ДЕПОЗИТНА АДРЕСА БІРЖІ"
        } else if senders.len() > 150 && recips.len() > 150 {
            "ГАРЯЧИЙ ГАМАНЕЦЬ (біржа/сервіс)"
        } else if top_is_cex {
            "шле на біржу (не воронка)"
        } else if n_in + n_out < 50 && senders.len() < 10 {
            "СХОВИЩЕ (мало операцій, великі суми)"
        } else { "звичайний гаманець" };

        eprintln!("{:<44} {:>5}вх {:>5}вих · відправників {:>4} · отримувачів {:>4} · воронка {:.2} · {}",
            a, n_in, n_out, senders.len(), recips.len(), funnel, verdict);
        out.push(json!({"addr": a, "in": n_in, "out": n_out, "senders": senders.len(),
            "recipients": recips.len(), "funnel": funnel, "topDest": top_dst,
            "topDestIsCex": top_is_cex, "topDestLabel": labels.get(&top_dst),
            "verdict": verdict}));
        if (i + 1) % 10 == 0 { eprintln!("--- {}/{}", i + 1, addrs.len()) }
    }

    let outp = format!("{bf}/eoacheck.json");
    let _ = File::create(&outp).unwrap()
        .write_all(serde_json::to_string(&json!({"rows": out})).unwrap().as_bytes());
    eprintln!("\n[eoacheck] збережено {outp}");
}
