//! txmass — усі транзакції по всіх мережах за весь час для великого списку гаманців.
//!
//! Призначення: зібрати базу по NET-WEAK (52 779 адрес), яку паралельна сесія не качає —
//! її `txfull` іде по 6 283 членах NET (mainnet, далі Arbitrum). Перетин виключається
//! автоматично: усе, що вже є в їхніх `.done`, пропускається.
//!
//! ДИСК — головне обмеження, а не швидкість. Їхній формат дає 2.4 МБ на гаманець, тобто
//! 52 779 адрес — це ~127 ГБ. На кореневому розділі вільно 37 ГБ, тож запис іде на окремий
//! диск (OUT), а запис компактний: із транзакції лишаються поля, потрібні для аналізу
//! потоків і позицій. Газ, nonce і статус викинуто — вони важать половину обсягу й у
//! жодному з наших аналізів не використовуються.
//!
//! Резюмованість обов'язкова: збір на добу не можна втрачати через один 429 чи перезапуск.
//! `.done` веде записи «мережа:гаманець», файл дописується, пам'ять не накопичується.
//!
//! Порядок мереж — за очікуваною віддачею: Ethereum, потім L2 і сайдчейни. Гаманець без
//! активності в мережі коштує один запит, тож дешеві мережі не шкодять.
//!
//! Args: ROOT WALLET_FILE. Env: OUT(/mnt/nvme-disk/txmass) CHAINS(...) CONC(6) MAXPAGES(10)
//!       SKIP_DONE(шляхи до чужих .done через кому)
//! Вихід: <OUT>/tx-<chain>.jsonl + <OUT>/tx.done

use serde_json::{json, Value};
use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::{BufWriter, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Semaphore;

/// (ключ, chainid). Порядок — за очікуваною віддачею.
const CHAINS: [(&str, u64); 10] = [
    ("eth", 1),
    ("arbitrum", 42161),
    ("base", 8453),
    ("polygon", 137),
    ("optimism", 10),
    ("bsc", 56),
    ("zksync", 324),
    ("linea", 59144),
    ("blast", 81457),
    ("gnosis", 100),
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

/// Etherscan під лімітом віддає HTTP **200** із тілом
/// `{"status":"0","message":"NOTOK","result":"Max rate limit reached"}`.
/// Реакція лише на код 429 тому не працює: 600 гаманців дали 41 401 рядок і НУЛЬ
/// позначок «зроблено», бо кожне обмеження зараховувалось як остаточний збій.
/// Ліміт треба ловити в ТІЛІ відповіді й повторювати.
async fn es(c: &reqwest::Client, url: &str) -> Option<Value> {
    for attempt in 0..7u64 {
        if let Ok(r) = c.get(url).timeout(Duration::from_secs(35)).send().await {
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
                        return Some(v);
                    }
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(500 * (1u64 << attempt.min(6)))).await;
    }
    None
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let mut a = std::env::args().skip(1);
    let root = a.next().unwrap_or_else(|| "/opt/wallet-tracker".into());
    let wallet_file = a.next().expect("вкажи файл зі списком гаманців");
    let out = std::env::var("OUT").unwrap_or_else(|_| "/mnt/nvme-disk/txmass".into());
    let conc: usize = envn("CONC", 6);
    let maxpages: u32 = envn("MAXPAGES", 10);
    let chains_env = std::env::var("CHAINS").unwrap_or_default();
    let keys: Vec<String> = envv(&root, "BACKFILL_ETHERSCAN_KEYS")
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    assert!(!keys.is_empty(), "BACKFILL_ETHERSCAN_KEYS порожній");
    fs::create_dir_all(&out).expect("не створити OUT");

    let chains: Vec<(&str, u64)> = if chains_env.is_empty() {
        CHAINS.to_vec()
    } else {
        CHAINS.iter().filter(|(k, _)| chains_env.split(',').any(|x| x.trim() == *k)).copied().collect()
    };

    let wallets: Vec<String> = fs::read_to_string(&wallet_file)
        .expect("wallet file")
        .lines()
        .map(|l| l.trim().to_lowercase())
        .filter(|l| l.starts_with("0x") && l.len() == 42)
        .collect();

    // Уже зроблене нами + уже зібране паралельною сесією.
    let done_path = format!("{out}/tx.done");
    let mut done: HashSet<String> = fs::read_to_string(&done_path)
        .map(|s| s.lines().map(|l| l.trim().to_string()).collect())
        .unwrap_or_default();
    let mut foreign = 0usize;
    for p in std::env::var("SKIP_DONE").unwrap_or_default().split(',') {
        let p = p.trim();
        if p.is_empty() {
            continue;
        }
        // Формат чужого .done — просто адреси (їхній txfull ведеться на одну мережу).
        let chain_of_file = if p.contains("-1.") { "eth" } else if p.contains("42161") { "arbitrum" } else { "" };
        if chain_of_file.is_empty() {
            continue;
        }
        if let Ok(s) = fs::read_to_string(p) {
            for l in s.lines() {
                let w = l.trim().to_lowercase();
                if w.starts_with("0x") {
                    done.insert(format!("{chain_of_file}:{w}"));
                    foreign += 1;
                }
            }
        }
    }
    println!(
        "[txmass] гаманців {} · мереж {} · уже зроблено {} (з них чужих {foreign})",
        wallets.len(),
        chains.len(),
        done.len()
    );

    let writers: Arc<Mutex<std::collections::HashMap<String, BufWriter<fs::File>>>> =
        Arc::new(Mutex::new(std::collections::HashMap::new()));
    let done_file = Arc::new(Mutex::new(
        OpenOptions::new().create(true).append(true).open(&done_path).expect("done"),
    ));
    let kc = Arc::new(AtomicUsize::new(0));
    let stat = Arc::new(AtomicUsize::new(0));
    let rows_total = Arc::new(AtomicUsize::new(0));
    let fails = Arc::new(AtomicUsize::new(0));

    for (chain, cid) in chains {
        let todo: Vec<String> = wallets
            .iter()
            .filter(|w| !done.contains(&format!("{chain}:{w}")))
            .cloned()
            .collect();
        println!("[txmass] {chain}: до збору {} з {}", todo.len(), wallets.len());
        let sem = Arc::new(Semaphore::new(conc));
        let mut futs = Vec::new();
        for w in todo {
            let (sem, keys, kc, writers, done_file, stat, rows_total, fails, out) = (
                sem.clone(),
                keys.clone(),
                kc.clone(),
                writers.clone(),
                done_file.clone(),
                stat.clone(),
                rows_total.clone(),
                fails.clone(),
                out.clone(),
            );
            let client = reqwest::Client::builder()
                .user_agent("wallet-radar-txmass/1.0")
                .build()
                .unwrap();
            futs.push(tokio::spawn(async move {
                // Семафор БЕРЕТЬСЯ ПЕРШИМ. У попередній версії він стояв після мережевих
                // викликів — і обмеження не працювало зовсім: усі 54 497 задач вистрілили
                // одночасно, Etherscan відповів лімітом майже на все, а `.done` записався
                // так, ніби гаманці порожні. 34 500 адрес «зібралися» за дві хвилини з
                // 2.6 рядка на гаманець, хоча перша ж перевірена адреса має транзакції.
                let _p = sem.acquire().await.ok()?;
                let mut rows: Vec<String> = Vec::new();
                let mut ok = true;
                // ext = звичайні, tok = ERC-20, int = внутрішні виклики
                for (kind, action) in [("ext", "txlist"), ("tok", "tokentx"), ("int", "txlistinternal")] {
                    for page in 1..=maxpages {
                        let key = &keys[kc.fetch_add(1, Ordering::Relaxed) % keys.len()];
                        let url = format!(
                            "https://api.etherscan.io/v2/api?chainid={cid}&module=account&action={action}\
                             &address={w}&page={page}&offset=1000&sort=asc&apikey={key}"
                        );
                        // Збій НЕ можна плутати з «немає транзакцій»: перше означає, що
                        // гаманець треба перепитати, друге — що він справді порожній.
                        let Some(d) = es(&client, &url).await else { ok = false; break };
                        let Some(arr) = d["result"].as_array() else {
                            // "No transactions found" — валідна відповідь, це не збій.
                            if d["message"].as_str() != Some("No transactions found") {
                                ok = false;
                            }
                            break;
                        };
                        let n = arr.len();
                        for t in arr {
                            // Компактний запис: газ, nonce і статус викинуто — вони важать
                            // половину обсягу й у наших аналізах не використовуються.
                            let r = json!({
                                "w": w,
                                "k": kind,
                                "ts": t["timeStamp"].as_str().unwrap_or(""),
                                "f": t["from"].as_str().unwrap_or("").to_lowercase(),
                                "t": t["to"].as_str().unwrap_or("").to_lowercase(),
                                "v": t["value"].as_str().unwrap_or(""),
                                "tok": t["contractAddress"].as_str().unwrap_or("").to_lowercase(),
                                "sym": t["tokenSymbol"].as_str().unwrap_or(""),
                                "dec": t["tokenDecimal"].as_str().unwrap_or(""),
                                "h": t["hash"].as_str().unwrap_or(""),
                            });
                            rows.push(r.to_string());
                        }
                        if n < 1000 {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(120)).await;
                    }
                }
                if !rows.is_empty() {
                    let mut ws = writers.lock().unwrap();
                    let f = ws.entry(chain.to_string()).or_insert_with(|| {
                        BufWriter::with_capacity(
                            1 << 20,
                            OpenOptions::new()
                                .create(true)
                                .append(true)
                                .open(format!("{out}/tx-{chain}.jsonl"))
                                .expect("вихідний файл"),
                        )
                    });
                    for r in &rows {
                        let _ = writeln!(f, "{r}");
                    }
                    let _ = f.flush();
                    rows_total.fetch_add(rows.len(), Ordering::Relaxed);
                }
                // Позначаємо зробленим ЛИШЕ те, що справді відповіло. Інакше resume
                // вважатиме недокачане завершеним, і дірку вже ніколи не видно.
                if ok {
                    let mut df = done_file.lock().unwrap();
                    let _ = writeln!(df, "{chain}:{w}");
                } else {
                    fails.fetch_add(1, Ordering::Relaxed);
                }
                let n = stat.fetch_add(1, Ordering::Relaxed) + 1;
                if n % 500 == 0 {
                    println!(
                        "[txmass] {chain}: {n} гаманців · рядків {} · збоїв {}",
                        rows_total.load(Ordering::Relaxed),
                        fails.load(Ordering::Relaxed)
                    );
                }
                Some(())
            }));
        }
        for f in futs {
            let _ = f.await;
        }
        println!("[txmass] {chain}: ГОТОВО");
        // Позначки цієї мережі більше не потрібні в памʼяті наступних ітерацій.
        done.retain(|k| !k.starts_with(&format!("{chain}:")));
    }
    println!("[txmass] УСЕ ГОТОВО · рядків: {}", rows_total.load(Ordering::Relaxed));
}
