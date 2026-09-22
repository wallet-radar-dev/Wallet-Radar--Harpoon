//! eth_edges2 — нативні ETH-ребра З СУМАМИ для ВСІХ відстежуваних гаманців.
//!
//! Навіщо, коли є eth_edges: (1) старий бін зберігав лише список контрагентів БЕЗ сум, тож
//! відрізнити реальний переказ від dust-спаму адресного отруєння (1e-9 ETH з адрес-двійників)
//! неможливо — через це в групи заходили випадкові кити; (2) він пройшов лише 3758 членів.
//! Навіщо взагалі краул: 19266 із 27281 відстежуваних НЕ мають жодного рядка в transfers.csv,
//! і вони вже позначені `done` у _progress.csv — тобто ERC-20 у них справді нема, backfill2
//! їх просто пропустить. Їхня активність — нативний ETH, якого бекфіл не збирав.
//!
//! Etherscan V2 chainid=1: txlist + txlistinternal. Резюмований (файл .done), APPEND.
//! Вихід: dataset/backfill/eth-edges2.jsonl
//!   {"w":адреса,"in":ETH,"out":ETH,"n":скільки tx,"first":ts,"last":ts,
//!    "cps":[{"a":контрагент,"i":ETH_вхід,"o":ETH_вихід,"n":к-сть,"last":ts}]}
//! Args: ROOT [WALLET_FILE]. Env: CONC(10) TOPCP(300) MINETH(0.0005)
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::Semaphore;

fn envv(root: &str, k: &str) -> String {
    fs::read_to_string(format!("{root}/.env")).unwrap_or_default().lines()
        .find(|l| l.starts_with(&format!("{k}="))).map(|l| l.splitn(2, '=').nth(1).unwrap_or("").trim().to_string()).unwrap_or_default()
}
/// API_BASE (напр. https://base.blockscout.com/api) вмикає Blockscout: той самий Etherscan-сумісний
/// формат, але БЕЗ ключа й chainid. Потрібен, бо безкоштовний ключ Etherscan V2 покриває лише
/// mainnet і Arbitrum: для Base/Optimism він відповідає "Free API access is not supported".
/// `Some(vec)` — валідна відповідь (порожній вектор = «No transactions found», історія справді порожня).
/// `None` — стійкий ліміт/мережа після всіх спроб; викликач НЕ має мітити гаманець як зроблений.
///
/// Чому Option: раніше поверталось `Vec<Value>`, і порожній вектор означав одночасно «немає tx»
/// і «здався». Etherscan під лімітом віддає HTTP **200** з тілом
/// `{"status":"0","message":"NOTOK","result":"Max rate limit reached"}` — тут `result` РЯДОК,
/// тож `as_array()` не спрацьовував, п'ять спроб вигорали, повертався `Vec::new()`, а викликач
/// безумовно писав гаманець у `.done`. Залімічений гаманець назавжди лишався з нулем рядків.
/// Скільки записів віддає Etherscan за один запит на безкоштовному рівні. Більше просити марно:
/// offset=2000/5000/10000 однаково повертають 1000.
const PAGE: usize = 1000;
/// Запобіжник від нескінченної пагінації (1000 × 400 = до 400k переказів на вид).
const MAX_WINDOWS: usize = 400;

async fn es(client: &reqwest::Client, keys: &[String], kc: &AtomicUsize, params: &str, chain: u32, api_base: &str) -> Option<Vec<Value>> {
    for attempt in 0..7u64 {
        let url = if api_base.is_empty() {
            let key = &keys[kc.fetch_add(1, Ordering::Relaxed) % keys.len()];
            format!("https://api.etherscan.io/v2/api?chainid={chain}&{params}&apikey={key}")
        } else { format!("{api_base}?{params}") };
        if let Ok(r) = client.get(url).send().await {
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
    let conc: usize = std::env::var("CONC").ok().and_then(|s| s.parse().ok()).unwrap_or(10);
    let topcp: usize = std::env::var("TOPCP").ok().and_then(|s| s.parse().ok()).unwrap_or(300);
    let min_eth: f64 = std::env::var("MINETH").ok().and_then(|s| s.parse().ok()).unwrap_or(0.0005);
    // CHAIN: 1=mainnet 42161=arbitrum 8453=base 10=optimism 324=zksync 137=polygon (Etherscan V2)
    let chain: u32 = std::env::var("CHAIN").ok().and_then(|s| s.parse().ok()).unwrap_or(1);
    let api_base = std::env::var("API_BASE").unwrap_or_default();
    let suffix = if chain == 1 { String::new() } else { format!("-{chain}") };
    let bf = format!("{root}/dataset/backfill");
    let keys: Vec<String> = envv(&root, "BACKFILL_ETHERSCAN_KEYS").split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    assert!(!keys.is_empty(), "нема BACKFILL_ETHERSCAN_KEYS");

    let wfile = std::env::args().nth(2).unwrap_or_else(|| format!("{bf}/backfill-wallets.txt"));
    let mut wallets: Vec<String> = fs::read_to_string(&wfile).expect("wallet file").lines()
        .map(|l| l.trim().to_lowercase()).filter(|l| l.len() == 42 && l.starts_with("0x")).collect();
    wallets.sort(); wallets.dedup();

    // спершу ті, про кого взагалі нема даних (порожні в transfers.csv) — саме вони закривають прогалину
    let mut have: HashSet<String> = HashSet::new();
    if let Ok(f) = File::open(format!("{bf}/transfers.csv")) {
        for line in BufReader::new(f).lines().map_while(Result::ok) {
            if let Some(w) = line.split(',').next() { if w.len() == 42 { have.insert(w.to_lowercase()); } }
        }
    }
    wallets.sort_by_key(|w| have.contains(w));   // false(нема даних) → перші

    let outp = format!("{bf}/eth-edges2{suffix}.jsonl");
    let donep = format!("{bf}/eth-edges2{suffix}.done");
    let mut done: HashSet<String> = HashSet::new();
    if let Ok(f) = File::open(&donep) { for l in BufReader::new(f).lines().map_while(Result::ok) { done.insert(l.trim().to_lowercase()); } }
    let todo: Vec<String> = wallets.iter().filter(|w| !done.contains(*w)).cloned().collect();
    eprintln!("[eth_edges2] мережа {chain} · гаманців: {} · без даних у бекфілі: {} · вже зроблено: {} · до краулу: {} (ключів {}, conc {})",
        wallets.len(), wallets.len() - have.len(), done.len(), todo.len(), keys.len(), conc);

    let keys = Arc::new(keys);
    let kc = Arc::new(AtomicUsize::new(0));
    let client = Arc::new(reqwest::Client::builder().user_agent("wallet-radar-edges2/0.1").pool_max_idle_per_host(conc).timeout(std::time::Duration::from_secs(40)).build().unwrap());
    let sem = Arc::new(Semaphore::new(conc));
    let out = Arc::new(Mutex::new(OpenOptions::new().create(true).append(true).open(&outp).unwrap()));
    let donef = Arc::new(Mutex::new(OpenOptions::new().create(true).append(true).open(&donep).unwrap()));
    let progress = Arc::new(AtomicUsize::new(0));
    let withdata = Arc::new(AtomicUsize::new(0));
    let total = todo.len();

    let mut handles = Vec::new();
    for w in todo {
        let (keys, kc, client, sem, out, donef, progress, withdata, api_base) =
            (keys.clone(), kc.clone(), client.clone(), sem.clone(), out.clone(), donef.clone(), progress.clone(), withdata.clone(), api_base.clone());
        handles.push(tokio::spawn(async move {
            let _p = sem.acquire().await.unwrap();
            let mut cps: HashMap<String, (f64, f64, u32, u64)> = HashMap::new();  // контрагент -> (вхід, вихід, к-сть, останній ts)
            let (mut tin, mut tout, mut ntx) = (0.0f64, 0.0f64, 0u32);
            let (mut first, mut last) = (u64::MAX, 0u64);
            let mut failed = false;
            // ПАГІНАЦІЯ ВІКНАМИ ПО БЛОКАХ. Раніше тут був ОДИН запит із offset=10000, і це мовчки
            // обрізало історію: Etherscan на безкоштовному рівні віддає РІВНО 1000 записів, хоч би
            // який offset просили (перевірено: 2000/5000/10000 -> 1000). Тобто в гаманця з понад
            // 1000 переказів бралась лише найдавніша тисяча, а решта ребер не існувала для
            // `grouppart` — при тому що саме цей бін дає йому ребра З СУМАМИ. Тепер беремо
            // партіями по 1000 і зсуваємо startblock на останній побачений блок.
            'kinds: for action in ["txlist", "txlistinternal"] {
                let mut from_block: u64 = 0;
                for _ in 0..MAX_WINDOWS {
                    let Some(txs) = es(&client, &keys, &kc, &format!("module=account&action={action}&address={w}&startblock={from_block}&endblock=99999999&page=1&offset={PAGE}&sort=asc"), chain, &api_base).await else {
                        failed = true;
                        break 'kinds;
                    };
                    if txs.is_empty() { break; }
                    let mut max_blk = from_block;
                    for t in &txs {
                        if let Some(b) = t["blockNumber"].as_str().and_then(|s| s.parse::<u64>().ok()) {
                            if b > max_blk { max_blk = b; }
                        }
                        if t["isError"].as_str() == Some("1") { continue; }
                        let eth: f64 = t["value"].as_str().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0) / 1e18;
                        if eth < min_eth { continue; }                       // дрібʼязок = спам, не звʼязок
                        let frm = t["from"].as_str().unwrap_or("").to_lowercase();
                        let to = t["to"].as_str().unwrap_or("").to_lowercase();
                        let ts: u64 = t["timeStamp"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0);
                        if ts > 0 { if ts < first { first = ts } if ts > last { last = ts } }
                        let is_out = frm == w;
                        let other = if is_out { &to } else { &frm };
                        if other.is_empty() || other == &w { continue; }
                        let e = cps.entry(other.clone()).or_insert((0.0, 0.0, 0, 0));
                        if is_out { e.1 += eth; tout += eth } else { e.0 += eth; tin += eth }
                        e.2 += 1; if ts > e.3 { e.3 = ts }
                        ntx += 1;
                    }
                    // остання сторінка; або блок не зрушив — далі ходити нікуди
                    if txs.len() < PAGE || max_blk <= from_block { break; }
                    from_block = max_blk;
                }
            }
            // При збої НЕ пишемо ні агрегат, ні done: рядок тут — це ПІДСУМОК по гаманцю
            // (сума in/out, список контрагентів), а не окремі транзакції. Частковий підсумок
            // виглядав би як повний і завищував/занижував би баланси у споживачів, які його
            // сумують. Гаманець просто лишається в черзі й перекачається з нуля наступним прогоном.
            if failed {
                let n = progress.fetch_add(1, Ordering::Relaxed) + 1;
                if n % 250 == 0 || n == total {
                    eprintln!("[eth_edges2] {n}/{total} · з активністю: {}", withdata.load(Ordering::Relaxed));
                }
                return;
            }
            if ntx > 0 {
                withdata.fetch_add(1, Ordering::Relaxed);
                let mut cv: Vec<(&String, &(f64, f64, u32, u64))> = cps.iter().collect();
                cv.sort_by(|a, b| (b.1 .0 + b.1 .1).partial_cmp(&(a.1 .0 + a.1 .1)).unwrap());
                cv.truncate(topcp);
                let rec = json!({"w": w, "in": tin, "out": tout, "n": ntx,
                    "first": if first == u64::MAX { 0 } else { first }, "last": last,
                    "cps": cv.iter().map(|(a, v)| json!({"a": a, "i": v.0, "o": v.1, "n": v.2, "last": v.3})).collect::<Vec<Value>>()});
                let mut o = out.lock().unwrap();
                let _ = writeln!(o, "{rec}");
            }
            { let mut d = donef.lock().unwrap(); let _ = writeln!(d, "{w}"); }
            let n = progress.fetch_add(1, Ordering::Relaxed) + 1;
            if n % 250 == 0 || n == total {
                eprintln!("[eth_edges2] {n}/{total} · з активністю: {}", withdata.load(Ordering::Relaxed));
            }
        }));
    }
    for h in handles { let _ = h.await; }
    eprintln!("[eth_edges2] ГОТОВО: пройдено {total}, з нативною ETH-активністю {}", withdata.load(Ordering::Relaxed));
}
