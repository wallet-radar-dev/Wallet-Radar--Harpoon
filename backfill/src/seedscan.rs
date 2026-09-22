//! seedscan — усі активи, яких торкалися задані гаманці, по всіх підтриманих мережах.
//!
//! Ідея: якщо ці адреси самі стежать за китами, то набір альткоїнів, у які вони заходили,
//! — це вже відфільтрований людьми список місць, де може сидіти непублічний капітал.
//! Шукати когорту в такому списку набагато дешевше, ніж перебирати топ-1000.
//!
//! Береться ІСТОРІЯ, а не поточні баланси: актив, з якого вже вийшли, інформативний так
//! само — саме там міг відбутися повний цикл накопичення й роздачі. Поточний баланс
//! записується окремим полем, щоб було видно, де вони ще сидять.
//!
//! Стейбли, обгортки й мости з результату виключені за тим самим правилом, що в grouppart:
//! спільний USDC нічого не означає, а WETH — це не позиція в активі.
//!
//! Args: ROOT [addr1 addr2 ...]. Env: SEEDS(через кому) MAXPAGES(6) CONC(6)
//! Вихід: dataset/backfill/seedscan.json + seedscan-report.md

use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;

/// (наш ключ, Etherscan chainid, Blockscout host)
const CHAINS: [(&str, u64, &str); 12] = [
    ("eth", 1, "https://eth.blockscout.com"),
    ("base", 8453, "https://base.blockscout.com"),
    ("arbitrum", 42161, "https://arbitrum.blockscout.com"),
    ("optimism", 10, "https://optimism.blockscout.com"),
    ("polygon", 137, "https://polygon.blockscout.com"),
    ("zksync", 324, "https://zksync.blockscout.com"),
    ("linea", 59144, "https://explorer.linea.build"),
    ("blast", 81457, "https://blast.blockscout.com"),
    ("scroll", 534352, "https://scroll.blockscout.com"),
    ("mantle", 5000, "https://mantle.blockscout.com"),
    ("gnosis", 100, "https://gnosis.blockscout.com"),
    ("celo", 42220, "https://celo.blockscout.com"),
];

fn envv(root: &str, k: &str) -> String {
    let Ok(s) = fs::read_to_string(format!("{root}/.env")) else { return String::new() };
    for line in s.lines() {
        if let Some(v) = line.strip_prefix(&format!("{k}=")) {
            return v.trim().to_string();
        }
    }
    String::new()
}
fn envn<T: std::str::FromStr>(k: &str, d: T) -> T {
    std::env::var(k).ok().and_then(|s| s.parse().ok()).unwrap_or(d)
}
fn num(v: &Value) -> f64 {
    match v {
        Value::Number(n) => n.as_f64().unwrap_or(0.0),
        Value::String(s) => s.parse().unwrap_or(0.0),
        _ => 0.0,
    }
}

/// Правило is_stable із grouppart: спільний USDC не є позицією.
fn is_boring(sym: &str) -> bool {
    let s = sym.to_uppercase();
    s.contains("USD")
        || s.contains("EUR")
        || s.contains("GBP")
        || matches!(
            s.as_str(),
            "DAI" | "FRAX" | "LUSD" | "TUSD" | "GHO" | "MIM" | "WBTC" | "CBBTC" | "TBTC"
                | "WETH" | "WBETH" | "CBETH" | "WSTETH" | "STETH" | "RETH" | "WEETH" | "EETH"
                | "ETH" | "WMATIC" | "WPOL" | "WBNB" | "WAVAX"
        )
}

async fn get_json(c: &reqwest::Client, url: &str) -> Option<Value> {
    // Ліміт Etherscan приходить як HTTP 200 з {"message":"NOTOK","result":"Max rate limit reached"} —
    // перевірки статусу замало, відказ треба ловити в ТІЛІ, інакше він читається як порожній результат.
    for attempt in 0..5u64 {
        if let Ok(r) = c.get(url).timeout(Duration::from_secs(30)).send().await {
            let st = r.status().as_u16();
            if st != 429 && st < 500 {
                if let Ok(v) = r.json::<Value>().await {
                    let msg = v["message"].as_str().unwrap_or("");
                    let res = v["result"].as_str().unwrap_or("");
                    let limited = msg == "NOTOK"
                        || res.contains("rate limit")
                        || res.contains("Max calls")
                        || res.contains("Too many");
                    if !limited { return Some(v); }
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(700 * (attempt + 1))).await;
    }
    None
}

#[derive(Default)]
struct Pos {
    symbol: String,
    chain: String,
    in_qty: f64,
    out_qty: f64,
    txs: usize,
    first: f64,
    last: f64,
    seeds: Vec<String>,
    held: f64,
    held_usd: f64,
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let mut a = std::env::args().skip(1);
    let root = a.next().unwrap_or_else(|| "/opt/wallet-tracker".into());
    let mut seeds: Vec<String> = a.map(|s| s.to_lowercase()).collect();
    if seeds.is_empty() {
        seeds = std::env::var("SEEDS")
            .unwrap_or_default()
            .split(',')
            .map(|s| s.trim().to_lowercase())
            .filter(|s| s.starts_with("0x"))
            .collect();
    }
    assert!(!seeds.is_empty(), "вкажи адреси");
    let dir = format!("{root}/dataset/backfill");
    let maxpages: u32 = envn("MAXPAGES", 6);
    let conc: usize = envn("CONC", 6);
    let keys: Vec<String> = envv(&root, "BACKFILL_ETHERSCAN_KEYS")
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    assert!(!keys.is_empty(), "BACKFILL_ETHERSCAN_KEYS порожній");

    let client = reqwest::Client::builder().user_agent("wallet-radar-seedscan/1.0").build().unwrap();
    let kc = Arc::new(AtomicUsize::new(0));
    let sem = Arc::new(Semaphore::new(conc));
    println!("[seedscan] сідів: {} · мереж: {}", seeds.len(), CHAINS.len());

    // ── історія ERC-20 по всіх мережах ──────────────────────────────────────
    let mut futs = Vec::new();
    for s in &seeds {
        for (chain, cid, _) in CHAINS {
            let (sem, client, kc, keys, s) =
                (sem.clone(), client.clone(), kc.clone(), keys.clone(), s.clone());
            futs.push(tokio::spawn(async move {
                let _p = sem.acquire().await.ok()?;
                let mut rows: Vec<Value> = Vec::new();
                for page in 1..=maxpages {
                    let key = &keys[kc.fetch_add(1, Ordering::Relaxed) % keys.len()];
                    let url = format!(
                        "https://api.etherscan.io/v2/api?chainid={cid}&module=account&action=tokentx\
                         &address={s}&page={page}&offset=1000&sort=asc&apikey={key}"
                    );
                    let Some(d) = get_json(&client, &url).await else { break };
                    let Some(arr) = d["result"].as_array() else { break };
                    let n = arr.len();
                    rows.extend(arr.iter().cloned());
                    if n < 1000 {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
                if !rows.is_empty() {
                    println!("[seedscan] {} {chain}: {} переказів", &s[..10], rows.len());
                }
                Some((s, chain, rows))
            }));
        }
    }

    let mut pos: HashMap<String, Pos> = HashMap::new(); // key = chain:contract
    for f in futs {
        let Ok(Some((s, chain, rows))) = f.await else { continue };
        for t in &rows {
            let c = t["contractAddress"].as_str().unwrap_or("").to_lowercase();
            if c.is_empty() {
                continue;
            }
            let sym = t["tokenSymbol"].as_str().unwrap_or("").to_string();
            if is_boring(&sym) {
                continue;
            }
            let dec = num(&t["tokenDecimal"]).clamp(0.0, 36.0);
            let amt = num(&t["value"]) / 10f64.powf(dec);
            if !amt.is_finite() || amt <= 0.0 {
                continue;
            }
            let ts = num(&t["timeStamp"]);
            let e = pos.entry(format!("{chain}:{c}")).or_insert_with(|| Pos {
                symbol: sym.clone(),
                chain: chain.to_string(),
                first: f64::MAX,
                ..Default::default()
            });
            e.txs += 1;
            e.first = e.first.min(ts);
            e.last = e.last.max(ts);
            if t["to"].as_str().unwrap_or("").to_lowercase() == s {
                e.in_qty += amt;
            } else {
                e.out_qty += amt;
            }
            if !e.seeds.contains(&s) {
                e.seeds.push(s.clone());
            }
        }
    }
    println!("[seedscan] різних активів у історії: {}", pos.len());

    // ── поточні баланси: де вони ще сидять ──────────────────────────────────
    for s in &seeds {
        for (chain, _, bs) in CHAINS {
            let Some(d) = get_json(&client, &format!("{bs}/api/v2/addresses/{s}/token-balances")).await
            else {
                continue;
            };
            for it in d.as_array().unwrap_or(&vec![]) {
                let t = &it["token"];
                let Some(c) = t["address_hash"].as_str().or_else(|| t["address"].as_str()) else {
                    continue;
                };
                let key = format!("{chain}:{}", c.to_lowercase());
                let dec = num(&t["decimals"]).clamp(0.0, 36.0);
                let amt = num(&it["value"]) / 10f64.powf(dec);
                let rate = num(&t["exchange_rate"]);
                if let Some(e) = pos.get_mut(&key) {
                    e.held += amt;
                    e.held_usd += amt * rate;
                }
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    // ── вихід ───────────────────────────────────────────────────────────────
    let mut v: Vec<(&String, &Pos)> = pos.iter().collect();
    // Спершу те, що торкнулися ОБИДВА сіди: перетин двох незалежних спостерігачів —
    // сильніший сигнал, ніж вибір одного.
    v.sort_by(|a, b| {
        b.1.seeds
            .len()
            .cmp(&a.1.seeds.len())
            .then(b.1.txs.cmp(&a.1.txs))
    });

    let both: Vec<&(&String, &Pos)> = v.iter().filter(|x| x.1.seeds.len() >= 2).collect();
    println!("[seedscan] активів, яких торкнулися ОБИДВА: {}", both.len());

    let js: Vec<Value> = v
        .iter()
        .map(|(k, p)| {
            json!({"key": k, "symbol": p.symbol, "chain": p.chain, "contract": k.split(':').nth(1).unwrap_or(""),
                   "seeds": p.seeds.len(), "txs": p.txs, "inQty": p.in_qty, "outQty": p.out_qty,
                   "heldQty": p.held, "heldUsd": p.held_usd, "stillIn": p.held > 0.0,
                   "firstTs": if p.first == f64::MAX { 0.0 } else { p.first }, "lastTs": p.last})
        })
        .collect();
    fs::write(
        format!("{dir}/seedscan.json"),
        serde_json::to_string_pretty(&json!({"seeds": seeds, "assets": js})).unwrap(),
    )
    .ok();

    let mut md = String::new();
    md.push_str("# Активи, яких торкалися задані спостерігачі\n\n");
    md.push_str(&format!(
        "Сідів: {} · мереж: {} · різних активів у історії: **{}** · торкнулися обидва: **{}**\n\n\
         Стейбли, обгортки й мости виключені. Береться вся історія переказів, а не лише \
         поточні баланси — актив, з якого вже вийшли, інформативний так само.\n\n",
        seeds.len(),
        CHAINS.len(),
        v.len(),
        both.len()
    ));
    md.push_str("| Актив | мережа | сідів | переказів | ще тримають | ~USD |\n|---|---|---:|---:|---|---:|\n");
    for (_, p) in v.iter().take(60) {
        md.push_str(&format!(
            "| {} | {} | {} | {} | {} | ${:.0} |\n",
            p.symbol,
            p.chain,
            p.seeds.len(),
            p.txs,
            if p.held > 0.0 { "так" } else { "—" },
            p.held_usd
        ));
    }
    fs::write(format!("{dir}/seedscan-report.md"), md).ok();
    println!("[seedscan] готово → {dir}/seedscan-report.md");
}
