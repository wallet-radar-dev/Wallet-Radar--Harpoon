//! funderkids — що роблять адреси, заправлені роздавачем газу.
//!
//! Привід: `0x4c18e3a2…` роздав по ~0.2 ETH на **143 адреси** (усього 31.3 ETH) — це не
//! торгівля, а заправка гаманців на газ. Одна з них (`0x02339afc…`) зібрала **76.8% емісії
//! CAMP** і не продала нічого. Питання: це поодинокий випадок чи конвеєр таких воронок?
//!
//! Для кожної адреси дивимось форму поведінки:
//!   * ВОРОНКА     — багато вхідних одного токена від різних, вихід ~0 (збирає й тримає);
//!   * РОЗДАВАЧ    — навпаки, багато вихідних на різні адреси;
//!   * ТОРГОВА     — обидва напрямки, кілька токенів;
//!   * СПЛЯЧА      — майже нічого не робила після заправки.
//! Плюс: скільки різних токенів, головний токен, чи є вихід ETH.
//!
//! Вихід: dataset/backfill/funderkids.json. Args: ROOT ADDR_FILE. Env: OFFSET(300)
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

async fn es(c: &reqwest::Client, keys: &[String], kc: &AtomicUsize, params: &str) -> Option<Vec<Value>> {
    for attempt in 0..5u64 {
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
        tokio::time::sleep(std::time::Duration::from_millis(400 * (1u64 << attempt.min(4)))).await;
    }
    None
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let bf = format!("{root}/dataset/backfill");
    let afile = std::env::args().nth(2).expect("вкажи файл з адресами");
    let offset: u32 = envn("OFFSET", 300);

    let keys: Vec<String> = envv(&root, "BACKFILL_ETHERSCAN_KEYS")
        .split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    assert!(!keys.is_empty());

    let members: HashSet<String> = BufReader::new(File::open(format!("{bf}/net-members.txt")).unwrap())
        .lines().map_while(Result::ok).map(|l| l.trim().to_lowercase()).collect();

    let addrs: Vec<String> = BufReader::new(File::open(&afile).unwrap()).lines()
        .map_while(Result::ok).map(|l| l.trim().to_lowercase())
        .filter(|l| l.len() == 42).collect();
    eprintln!("[funderkids] адрес: {}", addrs.len());

    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(40)).build().unwrap();
    let kc = AtomicUsize::new(0);
    let mut rows: Vec<Value> = Vec::new();
    let mut kinds: HashMap<&'static str, u32> = HashMap::new();
    let mut token_hits: HashMap<String, u32> = HashMap::new();

    for (i, a) in addrs.iter().enumerate() {
        let toks = es(&client, &keys, &kc,
            &format!("module=account&action=tokentx&address={a}&page=1&offset={offset}&sort=desc")).await
            .unwrap_or_default();
        let txs = es(&client, &keys, &kc,
            &format!("module=account&action=txlist&address={a}&page=1&offset={offset}&sort=desc")).await
            .unwrap_or_default();

        let mut sym_in: HashMap<String, f64> = HashMap::new();
        let mut senders: HashSet<String> = HashSet::new();
        let mut recips: HashSet<String> = HashSet::new();
        let (mut n_in, mut n_out) = (0u32, 0u32);
        for t in &toks {
            let f = t["from"].as_str().unwrap_or("").to_lowercase();
            let to = t["to"].as_str().unwrap_or("").to_lowercase();
            let sym = t["tokenSymbol"].as_str().unwrap_or("?").to_string();
            let dec: i32 = t["tokenDecimal"].as_str().and_then(|s| s.parse().ok()).unwrap_or(18);
            let amt = t["value"].as_str().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0)
                / 10f64.powi(dec);
            if to == *a {
                n_in += 1;
                senders.insert(f);
                *sym_in.entry(sym).or_insert(0.0) += amt;
            } else if f == *a {
                n_out += 1;
                recips.insert(to);
            }
        }
        let mut eth_out = 0u32;
        for t in &txs {
            if t["from"].as_str().unwrap_or("").to_lowercase() == *a
                && t["value"].as_str().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0) > 0.0 { eth_out += 1 }
        }
        let ntok = sym_in.len();
        let main_sym = sym_in.iter().max_by(|x, y| x.1.partial_cmp(y.1).unwrap())
            .map(|(s, _)| s.clone()).unwrap_or_default();

        // форма поведінки
        let kind = if n_in + n_out == 0 && eth_out == 0 { "СПЛЯЧА" }
            else if n_in >= 20 && senders.len() >= 10 && n_out <= 3 { "ВОРОНКА" }
            else if n_out >= 20 && recips.len() >= 10 && n_in <= 3 { "РОЗДАВАЧ" }
            else if n_in > 0 && n_out > 0 && ntok >= 2 { "ТОРГОВА" }
            else { "інше" };
        *kinds.entry(kind).or_insert(0) += 1;
        if kind == "ВОРОНКА" && !main_sym.is_empty() { *token_hits.entry(main_sym.clone()).or_insert(0) += 1 }

        eprintln!("{:<44} {:>4}вх {:>4}вих · відпр {:>3} · токенів {:>2} · {:<9} {} {}",
            a, n_in, n_out, senders.len(), ntok, kind, main_sym,
            if members.contains(a) { "[ЧЛЕН]" } else { "" });
        rows.push(json!({"addr": a, "in": n_in, "out": n_out, "senders": senders.len(),
            "recipients": recips.len(), "tokens": ntok, "mainToken": main_sym,
            "ethOut": eth_out, "kind": kind, "member": members.contains(a)}));
        if (i + 1) % 25 == 0 { eprintln!("--- {}/{}", i + 1, addrs.len()) }
    }

    eprintln!("\n=== ФОРМИ ПОВЕДІНКИ ===");
    let mut ks: Vec<(&&str, &u32)> = kinds.iter().collect();
    ks.sort_by(|a, b| b.1.cmp(a.1));
    for (k, v) in ks { eprintln!("  {:<10} {}", k, v) }
    if !token_hits.is_empty() {
        eprintln!("\n=== ЯКІ ТОКЕНИ ЗБИРАЮТЬ ВОРОНКИ ===");
        let mut th: Vec<(&String, &u32)> = token_hits.iter().collect();
        th.sort_by(|a, b| b.1.cmp(a.1));
        for (s, c) in th.iter().take(12) { eprintln!("  {:<12} {} адрес", s, c) }
    }
    let outp = format!("{bf}/funderkids.json");
    let _ = File::create(&outp).unwrap()
        .write_all(serde_json::to_string(&json!({"rows": rows})).unwrap().as_bytes());
    eprintln!("\n[funderkids] збережено {outp}");
}
