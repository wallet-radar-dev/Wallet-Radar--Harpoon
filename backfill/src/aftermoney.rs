//! aftermoney — ЩО РОБЛЯТЬ ЧЛЕНИ ПІСЛЯ ОТРИМАННЯ ГРОШЕЙ ВІД РОЗДАВАЧА.
//! Це найпрактичніше питання: якщо після виплати члени системно купують певні монети — маємо
//! випереджальний сигнал. Якщо несуть на біржу — це просто зарплата/кешаут.
//! Метод: беремо виплати роздавача (адреса+час), для кожного отримувача дивимось перші дії
//! у вікні WINDOW днів після виплати: купівля токена (ERC-20 прийшов з контракту), продаж/своп
//! (токен пішов на контракт), депозит на біржу, пересилання ETH далі, або нічого.
//! Вихід: dataset/backfill/aftermoney.json. Args: ROOT [DISPENSER]. Env: TOPN(150) WINDOW(7) CONC(6) PAGES(6)
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::BufReader;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::Semaphore;


/// Перша адреса з тегу — замість зашитого значення за замовчуванням.
/// Порожній результат краще, ніж мовчки взята стара адреса: видно одразу.
fn default_tagged(tag: &str) -> String {
    let Ok(c) = wr_store::open() else { return String::new() };
    let Ok(mut q) =
        c.prepare("SELECT address FROM wallet_tags WHERE tag=?1 ORDER BY address LIMIT 1")
    else {
        return String::new();
    };
    q.query_row([tag], |r| r.get(0)).unwrap_or_default()
}

fn envv(root: &str, k: &str) -> String {
    fs::read_to_string(format!("{root}/.env")).unwrap_or_default().lines()
        .find(|l| l.starts_with(&format!("{k}="))).map(|l| l.splitn(2, '=').nth(1).unwrap_or("").trim().to_string()).unwrap_or_default()
}
/// `Some(vec)` — валідна відповідь (порожній = «No transactions found»), `None` — стійкий ліміт.
///
/// Раніше обидва випадки давали `Vec::new()`: під лімітом Etherscan відповідає HTTP 200 з
/// `{"status":"0","message":"NOTOK","result":"Max rate limit reached"}`, де `result` — рядок,
/// тож `as_array()` не спрацьовував і чотири спроби мовчки вигорали. Для цього біна це означало
/// висновок «отримувач нічого не зробив із грошима» там, де його просто не вдалось перевірити.
async fn es(client: &reqwest::Client, keys: &[String], kc: &AtomicUsize, params: &str) -> Option<Vec<Value>> {
    for attempt in 0..7u64 {
        let key = &keys[kc.fetch_add(1, Ordering::Relaxed) % keys.len()];
        if let Ok(r) = client.get(format!("https://api.etherscan.io/v2/api?chainid=1&{params}&apikey={key}")).send().await {
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

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let disp = std::env::args().nth(2).unwrap_or_else(|| default_tagged("walletrole:dispensers")).to_lowercase();
    let topn: usize = std::env::var("TOPN").ok().and_then(|s| s.parse().ok()).unwrap_or(150);
    let window: u64 = std::env::var("WINDOW").ok().and_then(|s| s.parse().ok()).unwrap_or(7) * 86400;
    let conc: usize = std::env::var("CONC").ok().and_then(|s| s.parse().ok()).unwrap_or(6);
    let pages: u32 = std::env::var("PAGES").ok().and_then(|s| s.parse().ok()).unwrap_or(6);
    let bf = format!("{root}/dataset/backfill");
    let keys: Vec<String> = envv(&root, "BACKFILL_ETHERSCAN_KEYS").split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    assert!(!keys.is_empty());
    let kc = Arc::new(AtomicUsize::new(0));
    let client = Arc::new(reqwest::Client::builder().timeout(std::time::Duration::from_secs(40)).build().unwrap());
    let keys = Arc::new(keys);

    // довідники
    let lbl: serde_json::Map<String, Value> = serde_json::from_reader(BufReader::new(File::open(format!("{root}/dataset/labels-cache.json")).unwrap())).unwrap_or_default();
    let mut kind: HashMap<String, String> = HashMap::new();
    if let Ok(v) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(File::open(format!("{bf}/wallet-kind.json")).unwrap())) {
        for (a, k) in &v { kind.insert(a.to_lowercase(), k.as_str().unwrap_or("").to_string()); }
    }
    let is_cex = move |a: &str| matches!(kind.get(a).map(|s| s.as_str()), Some("exchange") | Some("otc"))
        || lbl.get(a).and_then(|x| x["name"].as_str()).map(|n| { let l = n.to_lowercase();
            ["binance","coinbase","okx","gate","kraken","bybit","kucoin","bitget","huobi","mexc","bitfinex","upbit","bithumb","paribu","btcturk","gopax","crypto.com","exchange","hot wallet","deposit"].iter().any(|c| l.contains(c)) }).unwrap_or(false);
    let is_cex = Arc::new(is_cex);

    // 1) виплати роздавача: отримувач -> (останній час, сума)
    let mut pay: HashMap<String, (u64, f64)> = HashMap::new();
    for p in 1..=pages {
        // Список виплат — основа всього біна. Обірвати його лімітом = мовчки звузити вибірку
        // отримувачів, тож зупиняємось гучно, а не робимо вигляд, що виплати закінчились.
        let Some(arr) = es(&client, &keys, &kc, &format!("module=account&action=txlist&address={disp}&page={p}&offset=1000&sort=desc")).await else {
            eprintln!("[aftermoney] ⚠ ліміт Etherscan обірвав історію роздавача на сторінці {p} — вибірка отримувачів неповна, результат не публікувати");
            std::process::exit(2);
        };
        let len = arr.len();
        for t in &arr {
            if t["from"].as_str().unwrap_or("").to_lowercase() != disp { continue; }
            let eth: f64 = t["value"].as_str().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0) / 1e18;
            if eth <= 0.0 { continue; }
            let to = t["to"].as_str().unwrap_or("").to_lowercase();
            let ts: u64 = t["timeStamp"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0);
            let e = pay.entry(to).or_insert((0, 0.0));
            if ts > e.0 { e.0 = ts; }
            e.1 += eth;
        }
        if len < 1000 { break; }
    }
    let mut recips: Vec<(String, u64, f64)> = pay.into_iter().map(|(a, (t, v))| (a, t, v)).collect();
    recips.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap());
    recips.truncate(topn);
    eprintln!("[aftermoney] отримувачів до перевірки: {} (вікно {} днів)", recips.len(), window / 86400);

    // 2) що робив кожен у вікні після виплати
    let sem = Arc::new(Semaphore::new(conc));
    let res: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let bought: Arc<Mutex<HashMap<String, usize>>> = Arc::new(Mutex::new(HashMap::new()));
    let mut hs = Vec::new();
    for (w, ts, got) in recips {
        let (client, keys, kc, sem, res, bought, is_cex) = (client.clone(), keys.clone(), kc.clone(), sem.clone(), res.clone(), bought.clone(), is_cex.clone());
        hs.push(tokio::spawn(async move {
            let _p = sem.acquire().await.unwrap();
            // Не змогли перевірити отримувача — не додаємо його в результат зовсім.
            // Інакше він потрапив би у звіт як «нічого не робив», спотворивши частки поведінок.
            let (Some(toks), Some(txs)) = (
                es(&client, &keys, &kc, &format!("module=account&action=tokentx&address={w}&page=1&offset=200&sort=desc")).await,
                es(&client, &keys, &kc, &format!("module=account&action=txlist&address={w}&page=1&offset=200&sort=desc")).await,
            ) else { return; };
            let (mut buy, mut sell, mut to_cex, mut fwd) = (0u32, 0u32, 0u32, 0u32);
            let mut syms: Vec<String> = Vec::new();
            for t in &toks {
                let tt: u64 = t["timeStamp"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0);
                if tt < ts || tt > ts + window { continue; }
                let from = t["from"].as_str().unwrap_or("").to_lowercase();
                let to = t["to"].as_str().unwrap_or("").to_lowercase();
                let sym = t["tokenSymbol"].as_str().unwrap_or("").to_string();
                if to == w { buy += 1; if !sym.is_empty() { syms.push(sym); } }
                else if from == w { if is_cex(&to) { to_cex += 1 } else { sell += 1 } }
            }
            for t in &txs {
                let tt: u64 = t["timeStamp"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0);
                if tt <= ts || tt > ts + window { continue; }
                if t["from"].as_str().unwrap_or("").to_lowercase() != w { continue; }
                let to = t["to"].as_str().unwrap_or("").to_lowercase();
                if t["value"].as_str() == Some("0") { continue; }
                if is_cex(&to) { to_cex += 1 } else { fwd += 1 }
            }
            let action = if to_cex > 0 { "на біржу" } else if buy > 0 { "купив токени" } else if sell > 0 { "продав/своп" } else if fwd > 0 { "переслав далі" } else { "нічого" };
            { let mut b = bought.lock().unwrap(); for s in syms { *b.entry(s).or_insert(0) += 1; } }
            res.lock().unwrap().push(json!({"w": w, "gotEth": got, "lastPayoutTs": ts, "buy": buy, "sell": sell, "toCex": to_cex, "forward": fwd, "action": action}));
        }));
    }
    for h in hs { let _ = h.await; }

    let r = res.lock().unwrap().clone();
    let mut agg: HashMap<&str, usize> = HashMap::new();
    for x in &r { *agg.entry(x["action"].as_str().unwrap_or("?")).or_insert(0) += 1; }
    let mut av: Vec<(&&str, &usize)> = agg.iter().collect(); av.sort_by(|a, b| b.1.cmp(a.1));
    eprintln!("[aftermoney] ===== ЩО РОБЛЯТЬ ПІСЛЯ ОТРИМАННЯ =====");
    for (k, v) in &av { eprintln!("   {:<14} {:>4} ({:.0}%)", k, v, 100.0 * **v as f64 / r.len().max(1) as f64); }
    let b = bought.lock().unwrap().clone();
    let mut bv: Vec<(&String, &usize)> = b.iter().collect(); bv.sort_by(|a, b| b.1.cmp(a.1));
    eprintln!("[aftermoney] --- які токени приходять у вікні (топ-12) ---");
    for (s, n) in bv.iter().take(12) { eprintln!("   {:<12} у {} отримувачів", s, n); }
    fs::write(format!("{bf}/aftermoney.json"), serde_json::to_string_pretty(&json!({
        "dispenser": disp, "windowDays": window / 86400, "recipients": r.len(),
        "actions": agg.iter().map(|(k, v)| json!({"action": k, "n": v})).collect::<Vec<Value>>(),
        "tokens": bv.iter().take(40).map(|(s, n)| json!({"sym": s, "recipients": n})).collect::<Vec<Value>>(),
        "detail": r})).unwrap()).ok();
    eprintln!("[aftermoney] збережено aftermoney.json");
}
