//! tokenpnl — хто historically заробив на кожному активі, чи сидить у ньому досі
//! і за якою середньою ціною заходив та виходив.
//!
//! Питання, на яке нарешті відповідають самі дані, без нуль-моделей: P&L вимірюється,
//! а не доводиться статистикою. По кожній парі «гаманець × токен» відновлюємо позицію з
//! повної історії переказів і оцінюємо кожен рух ціною ТОГО ДНЯ.
//!
//! Рішення, які тут прийняті свідомо:
//!   • Ціни — DefiLlama (денний ряд, один запит на токен). Ончейн-ціни в цьому проєкті вже
//!     ловили на похибці ×2350 по COMP, тож на них не спираємось узагалі. Токен без ряду
//!     цін не рахується, а йде в список `unpriced` — мовчки пропустити означало б збрехати
//!     про покриття.
//!   • База собівартості — СЕРЕДНЬОЗВАЖЕНА, не FIFO. FIFO дає інший реалізований P&L при
//!     тих самих даних і вимагає повної історії без пропусків; середня стійкіша до обрізаної
//!     історії, а обрізання тут неминуче (сторінка Etherscan = 1000 рядків).
//!   • Переказ ≠ купівля. Ми не бачимо, чи зайшов токен зі свопу, чи з власного гаманця.
//!     Тому вхід оцінюється як «набрав за ціною дня», і це ВЕРХНЯ межа точності: для адрес,
//!     що ганяють токен між своїми гаманцями, P&L буде завищено-шумний. Такі позначаються
//!     прапорцем `selfChurn`, коли контрагент входу й виходу збігається.
//!   • Резюмованість обовʼязкова: 12-18 год краулу не можна втрачати через один 429.
//!     Кожен токен після обробки дописується в .jsonl, його id — у .done.
//!
//! Args: ROOT. Env: TOPH(300) UNIVERSE(1000) CONC(10) DUST(1e-9) MAX_TOKENS(0=всі) RESET(0)
//! Вихід: dataset/backfill/tokenpnl.jsonl (по рядку на токен) + tokenpnl.done + tokenpnl-report.md

use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;

const CG: &str = "https://api.coingecko.com/api/v3";
const LLAMA: &str = "https://coins.llama.fi";

/// CoinGecko platform → (пріоритет, наш ключ, Blockscout host, Etherscan chainid, DefiLlama chain)
///
/// Пріоритет вирішує, ЯКИЙ деплой аналізувати, коли токен є в кількох мережах. Без нього
/// «перша платформа зі списку CG» дала USDT на Celo, USDC на Arbitrum і BNB на Ethereum —
/// тобто мостові обгортки замість канонічних контрактів, а отже й чужу історію торгів.
fn chain_of(cg_platform: &str) -> Option<(u8, &'static str, &'static str, u64, &'static str)> {
    Some(match cg_platform {
        "ethereum" => (0, "eth", "https://eth.blockscout.com", 1, "ethereum"),
        "base" => (1, "base", "https://base.blockscout.com", 8453, "base"),
        "arbitrum-one" => (2, "arbitrum", "https://arbitrum.blockscout.com", 42161, "arbitrum"),
        "optimistic-ethereum" => (3, "optimism", "https://optimism.blockscout.com", 10, "optimism"),
        "polygon-pos" => (4, "polygon", "https://polygon.blockscout.com", 137, "polygon"),
        "zksync" => (5, "zksync", "https://zksync.blockscout.com", 324, "era"),
        "linea" => (6, "linea", "https://explorer.linea.build", 59144, "linea"),
        "blast" => (7, "blast", "https://blast.blockscout.com", 81457, "blast"),
        "scroll" => (8, "scroll", "https://scroll.blockscout.com", 534352, "scroll"),
        "mantle" => (9, "mantle", "https://mantle.blockscout.com", 5000, "mantle"),
        "xdai" => (10, "gnosis", "https://gnosis.blockscout.com", 100, "xdai"),
        "celo" => (11, "celo", "https://celo.blockscout.com", 42220, "celo"),
        _ => return None,
    })
}

/// Наш ключ мережі → (Blockscout host, Etherscan chainid, DefiLlama chain).
/// Потрібен для режиму ASSETS_FILE: там мережа вже задана нашим ключем, а не
/// платформою CoinGecko.
fn chain_by_key(k: &str) -> Option<(&'static str, u64, &'static str)> {
    Some(match k {
        "eth" => ("https://eth.blockscout.com", 1, "ethereum"),
        "base" => ("https://base.blockscout.com", 8453, "base"),
        "arbitrum" => ("https://arbitrum.blockscout.com", 42161, "arbitrum"),
        "optimism" => ("https://optimism.blockscout.com", 10, "optimism"),
        "polygon" => ("https://polygon.blockscout.com", 137, "polygon"),
        "zksync" => ("https://zksync.blockscout.com", 324, "era"),
        "linea" => ("https://explorer.linea.build", 59144, "linea"),
        "blast" => ("https://blast.blockscout.com", 81457, "blast"),
        "scroll" => ("https://scroll.blockscout.com", 534352, "scroll"),
        "mantle" => ("https://mantle.blockscout.com", 5000, "mantle"),
        "gnosis" => ("https://gnosis.blockscout.com", 100, "xdai"),
        "celo" => ("https://celo.blockscout.com", 42220, "celo"),
        _ => return None,
    })
}

/// Спам-тікер видає себе сам: домен у назві, пробіли, дужки, заклик до дії.
/// Без цього фільтра у список потрапляють 273 фішингові ейрдропи з Polygon.
fn junk_symbol(s: &str) -> bool {
    let u = s.to_uppercase();
    if s.is_empty() || s.len() > 12 {
        return true;
    }
    if !s.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') {
        return true;
    }
    ["WWW", "HTTP", ".COM", ".IO", ".LY", "CLAIM", "REWARD", "AIRDROP", "VISIT", "EARN"]
        .iter()
        .any(|m| u.contains(m))
}

const CEX_SLUGS: [&str; 24] = [
    "binance", "coinbase", "kraken", "okx", "bybit", "kucoin", "huobi", "htx", "bitfinex",
    "upbit", "bithumb", "mexc", "bitget", "gate", "crypto-com", "bitstamp", "gemini",
    "poloniex", "bingx", "paribu", "btcturk", "exchange", "hot-wallet", "cold-wallet",
];
fn is_custodial(name: &str, tags: &Value) -> bool {
    let mut hay = name.to_lowercase();
    if let Some(arr) = tags.as_array() {
        for t in arr {
            hay.push(' ');
            hay.push_str(&t["slug"].as_str().unwrap_or("").to_lowercase());
            hay.push(' ');
            hay.push_str(&t["name"].as_str().unwrap_or("").to_lowercase());
        }
    }
    CEX_SLUGS.iter().any(|s| hay.contains(s))
}

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
fn is_burn(a: &str) -> bool {
    a.starts_with("0x000000000000000000000000000000000000")
}

async fn get_json(c: &reqwest::Client, url: &str, key: &str) -> Option<Value> {
    for attempt in 0..6u64 {
        let mut rq = c.get(url).timeout(Duration::from_secs(35));
        if url.starts_with(CG) && !key.is_empty() {
            rq = rq.header("x-cg-demo-api-key", key);
        }
        if let Ok(r) = rq.send().await {
            let st = r.status().as_u16();
            if st != 429 && st < 500 {
                if let Ok(v) = r.json::<Value>().await {
                    // Ліміт Etherscan приходить як HTTP 200 з тілом
                    // {"message":"NOTOK","result":"Max rate limit reached"}. У відповідях
                    // CoinGecko таких полів немає, тож перевірка їм не заважає.
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
        tokio::time::sleep(Duration::from_millis(600 * (1u64 << attempt.min(5)))).await;
    }
    None
}
fn qs(next: &Value) -> String {
    let Some(o) = next.as_object() else { return String::new() };
    let p: Vec<String> = o
        .iter()
        .map(|(k, v)| {
            let s = match v {
                Value::String(s) => s.clone(),
                o => o.to_string(),
            };
            format!("{k}={s}")
        })
        .collect();
    if p.is_empty() { String::new() } else { format!("?{}", p.join("&")) }
}

/// Найбільші рухачі токена за ВСЮ його історію, а не лише нинішні власники.
///
/// Навіщо: топ-холдери — це ті, хто ще НЕ вийшов. Питання «чи сидить він в активі
/// сьогодні» на такій вибірці має відповідь «так» у 90% випадків просто за побудовою,
/// а той, хто зафіксував прибуток і пішов, невидимий. Тому окремо семплюємо контрактні
/// перекази у W вікнах між першим і останнім блоком: повний скан історії великого токена
/// неможливий, а рівномірний зріз по часу ловить великі рухи в кожну епоху життя токена.
#[allow(clippy::too_many_arguments)]
async fn sample_movers(
    client: &reqwest::Client,
    keys: &[String],
    kc: &AtomicUsize,
    cid: u64,
    contract: &str,
    windows: u32,
    extra: usize,
) -> Vec<String> {
    let es = |params: &str, i: usize| {
        format!(
            "https://api.etherscan.io/v2/api?chainid={cid}&module=account&action=tokentx\
             &contractaddress={contract}&{params}&apikey={}",
            keys[i % keys.len()]
        )
    };
    let edge = |d: &Value| -> Option<u64> {
        d["result"].as_array()?.first()?["blockNumber"].as_str()?.parse().ok()
    };
    let f = get_json(client, &es("page=1&offset=10&sort=asc", kc.fetch_add(1, Ordering::Relaxed)), "")
        .await;
    let l = get_json(client, &es("page=1&offset=10&sort=desc", kc.fetch_add(1, Ordering::Relaxed)), "")
        .await;
    let (Some(first), Some(last)) = (f.as_ref().and_then(edge), l.as_ref().and_then(edge)) else {
        return Vec::new();
    };
    if last <= first {
        return Vec::new();
    }
    let step = ((last - first) / windows.max(1) as u64).max(1);
    // адреса → найбільший переказ, у якому вона брала участь
    let mut best: HashMap<String, f64> = HashMap::new();
    for w in 0..windows as u64 {
        let (a, b) = (first + w * step, first + (w + 1) * step);
        let url = es(
            &format!("startblock={a}&endblock={b}&page=1&offset=1000&sort=asc"),
            kc.fetch_add(1, Ordering::Relaxed),
        );
        let Some(d) = get_json(client, &url, "").await else { continue };
        let Some(arr) = d["result"].as_array() else { continue };
        for t in arr {
            let dec = num(&t["tokenDecimal"]).clamp(0.0, 36.0);
            let amt = num(&t["value"]) / 10f64.powf(dec);
            if !amt.is_finite() || amt <= 0.0 {
                continue;
            }
            for k in ["from", "to"] {
                let a = t[k].as_str().unwrap_or("").to_lowercase();
                if a.is_empty() || is_burn(&a) || a == contract {
                    continue;
                }
                let e = best.entry(a).or_insert(0.0);
                if amt > *e {
                    *e = amt;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    let mut v: Vec<(String, f64)> = best.into_iter().collect();
    v.sort_by(|x, y| y.1.partial_cmp(&x.1).unwrap());
    v.truncate(extra);
    v.into_iter().map(|x| x.0).collect()
}

/// Один переказ у стислому вигляді: усе, що потрібно для обліку позиції.
struct Mv {
    ts: f64,
    amt: f64,
    from: String,
    to: String,
}

/// Відсіяти контракти зі списку адрес, дібраних із історії переказів.
///
/// Холдери з Blockscout приходять із прапорцем `is_contract`, а `sample_movers` бере
/// адреси прямо з переказів — і там перевірки не було. Наслідок виміряний: серед 120
/// найбільших «заробітчан» 17 виявились контрактами (наприклад GRT `0x7f7929491f`
/// із «прибутком» $41.1М). Кеш спільний на весь прогін, бо ті самі маршрутизатори
/// трапляються в десятках токенів.
///
/// Пастка Etherscan: під лімітом він кладе ТЕКСТ помилки в `result`, тож валідною
/// вважається лише відповідь, що починається з "0x" — інакше повтор.
async fn filter_eoa(
    client: &reqwest::Client,
    keys: &[String],
    kc: &AtomicUsize,
    cid: u64,
    addrs: Vec<String>,
    cache: &mut HashMap<String, bool>,
    conc: usize,
) -> Vec<String> {
    let todo: Vec<String> = addrs.iter().filter(|a| !cache.contains_key(*a)).cloned().collect();
    if !todo.is_empty() {
        let sem = Arc::new(Semaphore::new(conc));
        let mut futs = Vec::new();
        for a in todo {
            let (sem, client, keys, kc) = (sem.clone(), client.clone(), keys.to_vec(), kc);
            let k = keys[kc.fetch_add(1, Ordering::Relaxed) % keys.len()].clone();
            futs.push(tokio::spawn(async move {
                let _p = sem.acquire().await.ok()?;
                let url = format!(
                    "https://api.etherscan.io/v2/api?chainid={cid}&module=proxy&action=eth_getCode\
                     &address={a}&tag=latest&apikey={k}"
                );
                let d = get_json(&client, &url, "").await?;
                let r = d["result"].as_str()?;
                r.starts_with("0x").then(|| (a, r.len() > 2))
            }));
        }
        for f in futs {
            if let Ok(Some((a, c))) = f.await {
                cache.insert(a, c);
            }
        }
    }
    // Невідоме трактуємо як контракт: краще втратити гаманець, ніж поставити
    // маршрутизатор у топ заробітчан.
    addrs.into_iter().filter(|a| cache.get(a) == Some(&false)).collect()
}

struct Tok {
    rank: usize,
    sym: String,
    name: String,
    chain: &'static str,
    bs: &'static str,
    cid: u64,
    llama: &'static str,
    contract: String,
    mcap: f64,
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let dir = format!("{root}/dataset/backfill");
    let toph: usize = envn("TOPH", 300);
    let universe: usize = envn("UNIVERSE", 1000);
    let conc: usize = envn("CONC", 10);
    let dust: f64 = envn("DUST", 1e-9);
    let max_tokens: usize = envn("MAX_TOKENS", 0);
    let windows: u32 = envn("WINDOWS", 10);
    let extra: usize = envn("EXTRA", 200);
    let dist_min: usize = envn("DIST_MIN", 10);
    let cgkey = envv(&root, "COINGECKO_API_KEY");
    let keys: Vec<String> = envv(&root, "BACKFILL_ETHERSCAN_KEYS")
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    assert!(!keys.is_empty(), "BACKFILL_ETHERSCAN_KEYS порожній");

    // Окремий тег виходу: результат по списку від сідів не має затирати прогін по топ-1000.
    let tag = std::env::var("OUT_TAG").unwrap_or_default();
    let out_path = format!("{dir}/tokenpnl{tag}.jsonl");
    let done_path = format!("{dir}/tokenpnl{tag}.done");
    if envn("RESET", 0) == 1 {
        let _ = fs::remove_file(&out_path);
        let _ = fs::remove_file(&done_path);
    }
    let done: HashSet<String> = fs::read_to_string(&done_path)
        .map(|s| s.lines().map(|l| l.trim().to_string()).collect())
        .unwrap_or_default();

    let client = reqwest::Client::builder()
        .user_agent("wallet-radar-tokenpnl/1.0")
        .build()
        .unwrap();

    // ── A0. Зовнішній список активів (режим сідів) ──────────────────────────
    //
    // Коли всесвіт задають не капіталізацією, а чиїмось вибором (наприклад, активи, яких
    // торкалися спостерігачі за китами), CoinGecko взагалі не потрібен: мережа й контракт
    // уже відомі. Капіталізації тут немає, тож перевірка priceSuspect не працює —
    // натомість спрацьовують власні запобіжники: без ряду цін або без холдерів актив
    // просто не рахується.
    let assets_file = std::env::var("ASSETS_FILE").unwrap_or_default();
    let file_toks: Option<Vec<Tok>> = if !assets_file.is_empty() {
        let min_seeds: f64 = envn("MIN_SEEDS", 1.0);
        let src: Value =
            serde_json::from_str(&fs::read_to_string(&assets_file).expect("ASSETS_FILE")).expect("json");
        let mut toks: Vec<Tok> = Vec::new();
        for (i, a) in src["assets"].as_array().unwrap_or(&vec![]).iter().enumerate() {
            let sym = a["symbol"].as_str().unwrap_or("").to_string();
            let chain = a["chain"].as_str().unwrap_or("");
            let contract = a["contract"].as_str().unwrap_or("").to_lowercase();
            if junk_symbol(&sym) || contract.len() != 42 || num(&a["seeds"]) < min_seeds {
                continue;
            }
            let Some((bs, cid, llama)) = chain_by_key(chain) else { continue };
            toks.push(Tok {
                rank: i + 1,
                sym,
                name: String::new(),
                chain: Box::leak(chain.to_string().into_boxed_str()),
                bs,
                cid,
                llama,
                contract,
                mcap: 0.0,
            });
        }
        if max_tokens > 0 && toks.len() > max_tokens {
            toks.truncate(max_tokens);
        }
        println!("[tokenpnl] режим списку: {} активів із {assets_file}", toks.len());
        Some(toks)
    } else {
        None
    };

    // ── A. Всесвіт: топ-N за капіталізацією, розкладений по мережах ─────────
    let mut markets: Vec<Value> = Vec::new();
    for p in (1..=universe.div_ceil(250)).take_while(|_| file_toks.is_none()) {
        let url =
            format!("{CG}/coins/markets?vs_currency=usd&order=market_cap_desc&per_page=250&page={p}");
        if let Some(Value::Array(a)) = get_json(&client, &url, &cgkey).await {
            markets.extend(a);
        }
        tokio::time::sleep(Duration::from_millis(2500)).await;
    }
    markets.truncate(universe);
    let list = if file_toks.is_none() {
        get_json(&client, &format!("{CG}/coins/list?include_platform=true"), &cgkey)
            .await
            .unwrap_or(Value::Null)
    } else {
        Value::Null
    };
    let mut plats: HashMap<String, Vec<(String, String)>> = HashMap::new();
    if let Some(a) = list.as_array() {
        for c in a {
            let Some(id) = c["id"].as_str() else { continue };
            let Some(o) = c["platforms"].as_object() else { continue };
            let v: Vec<(String, String)> = o
                .iter()
                .filter_map(|(p, addr)| {
                    let a = addr.as_str()?;
                    (a.starts_with("0x") && a.len() == 42)
                        .then(|| (p.clone(), a.to_lowercase()))
                })
                .collect();
            if !v.is_empty() {
                plats.insert(id.to_string(), v);
            }
        }
    }

    let mut toks: Vec<Tok> = Vec::new();
    let mut uncovered: Vec<Value> = Vec::new();
    for (i, m) in markets.iter().enumerate() {
        let Some(id) = m["id"].as_str() else { continue };
        let sym = m["symbol"].as_str().unwrap_or("").to_uppercase();
        let name = m["name"].as_str().unwrap_or("").to_string();
        let mut placed = false;
        if let Some(ps) = plats.get(id) {
            // Одна мережа на токен — з найвищим пріоритетом серед підтриманих.
            let best = ps
                .iter()
                .filter_map(|(p, addr)| chain_of(p).map(|c| (c, addr)))
                .min_by_key(|((pri, ..), _)| *pri);
            if let Some(((_, chain, bs, cid, llama), addr)) = best {
                toks.push(Tok {
                    rank: i + 1,
                    sym: sym.clone(),
                    name: name.clone(),
                    chain,
                    bs,
                    cid,
                    llama,
                    contract: addr.clone(),
                    mcap: num(&m["market_cap"]),
                });
                placed = true;
            }
        }
        if !placed {
            uncovered.push(json!({"rank": i+1, "symbol": sym, "name": name,
                                  "platforms": plats.get(id).map(|v| v.iter().map(|x| x.0.clone()).collect::<Vec<String>>()).unwrap_or_default()}));
        }
    }
    // У режимі списку всесвіт уже зібраний вище — беремо його замість порожнього.
    let mut toks = file_toks.unwrap_or(toks);
    if max_tokens > 0 && toks.len() > max_tokens {
        toks.truncate(max_tokens);
    }
    fs::write(
        format!("{dir}/tokenpnl-uncovered.json"),
        serde_json::to_string_pretty(&json!({"count": uncovered.len(), "assets": uncovered})).unwrap(),
    )
    .ok();
    println!(
        "[tokenpnl] топ-{universe}: покрито {} токенів на EVM · поза покриттям {} (Solana/Tron/TON/BTC тощо)",
        toks.len(),
        uncovered.len()
    );
    println!("[tokenpnl] вже зроблено раніше: {}", done.len());

    let known: HashMap<String, String> = fs::read_to_string(format!("{dir}/group-membership.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v["wallets"].as_object().cloned())
        .map(|o| {
            o.into_iter()
                .filter_map(|(k, v)| v["group"].as_str().map(|g| (k.to_lowercase(), g.to_string())))
                .collect()
        })
        .unwrap_or_default();

    // Запасне джерело емісії — те, що вже зібрано проєктом (Ethereum).
    let local_supply: HashMap<String, f64> = fs::read_to_string(format!("{root}/dataset/token-supply.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v.as_object().cloned())
        .map(|o| {
            o.into_iter()
                .filter_map(|(k, v)| {
                    let dec = num(&v["decimals"]).clamp(0.0, 36.0);
                    let s = num(&v["supply"]) / 10f64.powf(dec);
                    (s > 0.0).then(|| (k.to_lowercase(), s))
                })
                .collect()
        })
        .unwrap_or_default();
    println!("[tokenpnl] локальних записів емісії: {}", local_supply.len());

    let kc = Arc::new(AtomicUsize::new(0));
    let total = toks.len();
    // Кеш ознаки контракту живе весь прогін і зберігається на диск: ті самі
    // маршрутизатори трапляються в десятках токенів, перепитувати їх марно.
    let eoa_path = format!("{dir}/eoa-cache{tag}.json");
    let mut eoa_cache: HashMap<String, bool> = fs::read_to_string(&eoa_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    println!("[tokenpnl] кеш EOA: {} адрес", eoa_cache.len());

    // ── B..E. Токен за токеном (резюмовано) ─────────────────────────────────
    for (ti, t) in toks.iter().enumerate() {
        let tid = format!("{}:{}", t.chain, t.contract);
        if done.contains(&tid) {
            continue;
        }

        // ціновий ряд: без нього рахувати точку входу нічим
        // DefiLlama віддає максимум 500 точок на запит, тож 5.5 років історії збираємо
        // чотирма вікнами по 500 днів. Без глибокої історії «середня точка входу» була б
        // порахована лише по останніх 16 місяцях — тобто неправдою для старих позицій.
        let now_ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as f64)
            .unwrap_or(0.0);
        let key_ll = format!("{}:{}", t.llama, t.contract);
        let mut series: Vec<(f64, f64)> = Vec::new();
        for k in (0..4u32).rev() {
            let start = now_ts - (k as f64 + 1.0) * 500.0 * 86400.0;
            let url = format!(
                "{LLAMA}/chart/{key_ll}?start={:.0}&span=500&period=1d&searchWidth=12h",
                start.max(1_500_000_000.0)
            );
            if let Some(c) = get_json(&client, &url, "").await {
                if let Some(arr) = c["coins"][&key_ll]["prices"].as_array() {
                    for p in arr {
                        series.push((num(&p["timestamp"]), num(&p["price"])));
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        series.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        series.dedup_by(|a, b| a.0 == b.0);
        if series.len() < 10 {
            append_line(&out_path, &json!({"token": tid, "symbol": t.sym, "unpriced": true}));
            append_line_raw(&done_path, &tid);
            println!("[tokenpnl] {}/{} {} — нема ряду цін, пропуск", ti + 1, total, t.sym);
            continue;
        }
        let price_at = |ts: f64| -> f64 {
            match series.binary_search_by(|p| p.0.partial_cmp(&ts).unwrap()) {
                Ok(i) => series[i].1,
                Err(0) => series[0].1,
                Err(i) if i >= series.len() => series[series.len() - 1].1,
                Err(i) => {
                    let (a, b) = (series[i - 1], series[i]);
                    if (ts - a.0) <= (b.0 - ts) { a.1 } else { b.1 }
                }
            }
        };
        let price_now = series[series.len() - 1].1;

        // Емісія потрібна, щоб відрізнити «заробив» від «випустив»: у пробі топ P&L
        // зайняли WBT ($2.2 млрд) і RAIN ($262 млн) — це не прибуток, а казна емітента,
        // яка ніколи нічого не купувала. Контракти ми вже відсіяли, але казна буває EOA.
        // ВАЖЛИВО: збій цього запиту НЕ має тихо вимикати фільтр. Саме так і сталося на
        // WBT — Blockscout не віддав емісію, supply став 0, `treasurySuspect` перестав
        // спрацьовувати, і гаманець із 60 млн токенів (~42% емісії) пройшов як трейдер
        // із «прибутком» $1.58 млрд. Тому: спроба, потім локальний файл, потім явна
        // ознака supplyUnknown і запасний поріг за абсолютною сумою.
        let mut supply = 0.0f64;
        for _ in 0..2 {
            if let Some(d) =
                get_json(&client, &format!("{}/api/v2/tokens/{}", t.bs, t.contract), "").await
            {
                let dec = num(&d["decimals"]).clamp(0.0, 36.0);
                let s = num(&d["total_supply"]) / 10f64.powf(dec);
                if s > 0.0 {
                    supply = s;
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        if supply <= 0.0 {
            if let Some(v) = local_supply.get(&t.contract) {
                supply = *v;
            }
        }
        let supply_unknown = supply <= 0.0;
        if supply_unknown {
            println!("[tokenpnl] {} — емісія невідома, працює запасний поріг казни", t.sym);
        }

        // холдери
        let mut holders: Vec<String> = Vec::new();
        let mut cex_set: HashSet<String> = HashSet::new();
        let mut params = String::new();
        for _ in 0..(toph.div_ceil(50) + 1) {
            if holders.len() >= toph {
                break;
            }
            let Some(d) =
                get_json(&client, &format!("{}/api/v2/tokens/{}/holders{params}", t.bs, t.contract), "")
                    .await
            else {
                break;
            };
            let Some(items) = d["items"].as_array().cloned() else { break };
            if items.is_empty() {
                break;
            }
            for it in &items {
                let w = it["address"]["hash"].as_str().unwrap_or("").to_lowercase();
                if is_burn(&w) || w == t.contract {
                    continue;
                }
                // Пули, казни й мости — це не «хтось заробив»: їхній P&L лише
                // відображає рух чужих грошей. У пробному прогоні саме вони дали
                // RAIN із «прибутком» $8.4 млрд. Біржі лишаємо, але з міткою.
                if it["address"]["is_contract"].as_bool().unwrap_or(false) {
                    continue;
                }
                let cex = is_custodial(
                    it["address"]["name"].as_str().unwrap_or(""),
                    &it["address"]["metadata"]["tags"],
                );
                holders.push(w);
                if cex {
                    cex_set.insert(holders[holders.len() - 1].clone());
                }
                if holders.len() >= toph {
                    break;
                }
            }
            let n = &d["next_page_params"];
            if n.is_null() {
                break;
            }
            params = qs(n);
            tokio::time::sleep(Duration::from_millis(120)).await;
        }
        let n_current = holders.len();
        // Ті, хто вже вийшов: без них питання «чи сидить досі» не має сенсу.
        let movers_raw = sample_movers(&client, &keys, &kc, t.cid, &t.contract, windows, extra).await;
        let movers = filter_eoa(&client, &keys, &kc, t.cid, movers_raw, &mut eoa_cache, conc).await;
        {
            let have: HashSet<String> = holders.iter().cloned().collect();
            for m in movers {
                if !have.contains(&m) {
                    holders.push(m);
                }
            }
        }
        if holders.is_empty() {
            append_line(&out_path, &json!({"token": tid, "symbol": t.sym, "noHolders": true}));
            append_line_raw(&done_path, &tid);
            continue;
        }
        let current_set: HashSet<String> = holders.iter().take(n_current).cloned().collect();

        // історія кожного холдера саме по цьому токену
        let sem = Arc::new(Semaphore::new(conc));
        let mut futs = Vec::new();
        for w in holders.clone() {
            let (sem, client, kc, keys) = (sem.clone(), client.clone(), kc.clone(), keys.clone());
            let (cid, contract) = (t.cid, t.contract.clone());
            futs.push(tokio::spawn(async move {
                let _p = sem.acquire().await.ok()?;
                // Одна сторінка = 1000 рядків. У пробному прогоні кожен другий великий
                // гаманець виходив із прапорцем truncated, а обрізана історія ламає
                // облік позиції: непорахований вхід робить із продажу «безкоштовний
                // прибуток». Тому гортаємо до MAXPAGES.
                let maxp: u32 = envn("MAXPAGES", 6);
                let mut arr: Vec<Value> = Vec::new();
                let mut truncated = false;
                for page in 1..=maxp {
                    let key = &keys[kc.fetch_add(1, Ordering::Relaxed) % keys.len()];
                    let url = format!(
                        "https://api.etherscan.io/v2/api?chainid={cid}&module=account&action=tokentx\
                         &contractaddress={contract}&address={w}&page={page}&offset=1000&sort=asc&apikey={key}"
                    );
                    let Some(d) = get_json(&client, &url, "").await else { break };
                    let Some(a) = d["result"].as_array() else { break };
                    let n = a.len();
                    arr.extend(a.iter().cloned());
                    if n < 1000 {
                        break;
                    }
                    if page == maxp {
                        truncated = true;
                    }
                    tokio::time::sleep(Duration::from_millis(150)).await;
                }
                // Тримати сирі JSON-обʼєкти всієї популяції токена дорого: 500 гаманців
                // × 6000 переказів дали 4.8 ГБ RSS, а на машині лишалось 3.9 ГБ вільних —
                // OOM-killer обрав би найбільший процес, тобто прод-бекенд. З кожного
                // переказу потрібні п'ять полів, тож згортаємо одразу.
                let compact: Vec<Mv> = arr
                    .iter()
                    .filter_map(|t| {
                        let dec = num(&t["tokenDecimal"]).clamp(0.0, 36.0);
                        let amt = num(&t["value"]) / 10f64.powf(dec);
                        if !amt.is_finite() || amt <= 0.0 {
                            return None;
                        }
                        Some(Mv {
                            ts: num(&t["timeStamp"]),
                            amt,
                            from: t["from"].as_str().unwrap_or("").to_lowercase(),
                            to: t["to"].as_str().unwrap_or("").to_lowercase(),
                        })
                    })
                    .collect();
                Some((w, compact, truncated))
            }));
        }

        // Спершу збираємо сирі підсумки, і лише потім судимо про роздачу: щоб побачити
        // «один відправник → десятки гаманців того самого дня», треба мати перед очима
        // ВСЮ популяцію токена, а не кожен гаманець окремо.
        let mut raw: Vec<(String, Vec<Mv>, bool)> = Vec::new();
        for f in futs {
            if let Ok(Some(x)) = f.await {
                raw.push(x);
            }
        }
        // (відправник, доба) → скільки різних гаманців популяції отримали від нього того дня
        let mut dist: HashMap<(String, i64), HashSet<String>> = HashMap::new();
        for (w, arr, _) in &raw {
            for t2 in arr {
                if t2.to != *w || t2.from.is_empty() {
                    continue;
                }
                let day = (t2.ts / 86400.0) as i64;
                dist.entry((t2.from.clone(), day)).or_default().insert(w.clone());
            }
        }

        let mut rows: Vec<Value> = Vec::new();
        for (w, arr, truncated) in raw {
            if arr.is_empty() {
                continue;
            }
            let (mut in_qty, mut in_usd, mut out_qty, mut out_usd) = (0.0f64, 0.0, 0.0f64, 0.0);
            let (mut first, mut last) = (f64::MAX, 0.0f64);
            let (mut dist_qty, mut dist_peers, mut dist_src) = (0.0f64, 0usize, String::new());
            let mut cps_in: HashSet<String> = HashSet::new();
            let mut cps_out: HashSet<String> = HashSet::new();
            for t2 in &arr {
                let (amt, ts) = (t2.amt, t2.ts);
                // І Etherscan, і DefiLlama дають секунди — жодних перерахунків.
                // (Був хибний варіант із .max() по двох масштабах: він систематично
                // завищував ціну входу, обираючи більшу з двох.)
                let px = price_at(ts);
                let (from, to) = (t2.from.clone(), t2.to.clone());
                if from == to {
                    continue;
                }
                first = first.min(ts);
                last = last.max(ts);
                if to == w {
                    in_qty += amt;
                    in_usd += amt * px;
                    // Роздача: цей самий відправник того самого дня наділив ще багатьох.
                    // Такий «вхід» не є купівлею, і середня ціна тут лише фіксує курс дня.
                    let day = (ts / 86400.0) as i64;
                    let peers = dist
                        .get(&(from.clone(), day))
                        .map(|s| s.len())
                        .unwrap_or(1);
                    if peers >= dist_min {
                        dist_qty += amt;
                        dist_peers = dist_peers.max(peers);
                        dist_src = from.clone();
                    }
                    cps_in.insert(from);
                } else if from == w {
                    out_qty += amt;
                    out_usd += amt * px;
                    cps_out.insert(to);
                }
            }
            if in_qty <= dust {
                continue;
            }
            let avg_in = in_usd / in_qty;
            let avg_out = if out_qty > dust { Some(out_usd / out_qty) } else { None };
            let realized = out_usd - avg_in * out_qty;
            let held = (in_qty - out_qty).max(0.0);
            let unrealized = held * (price_now - avg_in);
            let self_churn = !cps_in.is_disjoint(&cps_out);
            rows.push(json!({
                "wallet": w,
                "group": known.get(&w).cloned().unwrap_or_default(),
                "cex": cex_set.contains(&w),
                "inQty": in_qty, "outQty": out_qty,
                "avgEntryUsd": avg_in, "avgExitUsd": avg_out,
                "realizedUsd": realized, "unrealizedUsd": unrealized,
                "totalPnlUsd": realized + unrealized,
                "heldQty": held, "heldUsd": held * price_now,
                // Фаза 0.2: частка емісії — зі СЛОВНИКА. emission_share(x,s)*100 бітово тотожне
                // старому `if s>0 {x/s*100} else {0}`: для s>0 той самий порядок (x/s)*100, для s<=0 → 0.
                "heldPctOfSupply": wr_dict::emission_share(held, supply) * 100.0,
                "inPctOfSupply": wr_dict::emission_share(in_qty, supply) * 100.0,
                // Хто провів через себе понад 5% емісії — це роздавач/казна, а не трейдер.
                // Без відомої емісії судимо за абсолютною сумою: $25М в одному токені на
                // одному гаманці — це вже не роздрібна позиція.
                "supplyUnknown": supply_unknown,
                "treasurySuspect": if supply > 0.0 {
                    wr_dict::emission_share(held, supply) > 0.02 || wr_dict::emission_share(in_qty, supply) > 0.05
                } else {
                    held * price_now > 25_000_000.0 || in_qty * price_now > 50_000_000.0
                },
                "stillIn": held > dust,
                // Чи був цей гаманець у поточному топі холдерів, чи його дістали з історії:
                // без цього поділу «сидить досі» неможливо чесно інтерпретувати.
                "fromCurrentTop": current_set.contains(&w),
                "viaDistribution": dist_qty > 0.0 && dist_qty / in_qty >= 0.5,
                "distributionPctOfIn": if in_qty > 0.0 { dist_qty / in_qty * 100.0 } else { 0.0 },
                "distributionPeers": dist_peers,
                "distributionSource": dist_src,
                "firstTs": if first == f64::MAX { 0.0 } else { first }, "lastTs": last,
                "selfChurn": self_churn,
                "historyTruncated": truncated,
            }));
        }
        rows.sort_by(|a, b| num(&b["totalPnlUsd"]).partial_cmp(&num(&a["totalPnlUsd"])).unwrap());

        append_line(
            &out_path,
            &json!({
                "token": tid, "symbol": t.sym, "name": t.name, "chain": t.chain,
                "contract": t.contract, "rank": t.rank, "priceNow": price_now, "supply": supply,
                "marketCap": t.mcap,
                // Звірка «емісія × ціна» з капіталізацією CoinGecko. Розбіжність у рази
                // означає, що або ціна не від цього токена, або supply включає невипущене —
                // у пробі RAIN давав $15 млрд «капіталізації» й P&L $262М на цій підставі.
                // Проєкт уже горів на ончейн-цінах (COMP ×2350), тож токен не викидаємо,
                // а позначаємо: рішення за такими цифрами ухвалювати не можна.
                "impliedCap": supply * price_now,
                "priceSuspect": t.mcap > 0.0 && supply > 0.0
                    && (supply * price_now > t.mcap * 5.0 || supply * price_now < t.mcap / 5.0),
                "holdersScanned": holders.len(), "walletsWithHistory": rows.len(),
                "wallets": rows,
            }),
        );
        append_line_raw(&done_path, &tid);
        fs::write(&eoa_path, serde_json::to_string(&eoa_cache).unwrap_or_default()).ok();
        if (ti + 1) % 10 == 0 || ti < 5 {
            println!("[tokenpnl] {}/{} {} ({}) — гаманців із історією {}", ti + 1, total, t.sym, t.chain, rows.len());
        }
    }
    println!("[tokenpnl] краул завершено → {out_path}");
}

fn append_line(path: &str, v: &Value) {
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{v}");
    }
}
fn append_line_raw(path: &str, s: &str) {
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{s}");
    }
}

#[cfg(test)]
mod parity_tests {
    /// БАЙТ-ПАРИТЕТ (Фаза 0.2): міграція heldPctOfSupply/inPctOfSupply/treasurySuspect
    /// на wr_dict::emission_share нічого не змінює побітно.
    #[test]
    fn emission_share_bit_identical() {
        let sups: [f64; 6] = [1.0, 1.5, 1e6, 528_786_477.77, 1e18, 183_987_824_878.0];
        let qty: [f64; 6] = [0.0, 1.0, 250_000.0, 1e9, 9.9e17, 1.23456789e12];
        for &s in &sups {
            for &x in &qty {
                // pct-поля: ternary `if s>0 {x/s*100} else {0}` ≡ emission_share(x,s)*100 (s тут >0)
                assert_eq!((x / s * 100.0).to_bits(), (wr_dict::emission_share(x, s) * 100.0).to_bits());
                // порогові порівняння treasurySuspect: x/s ≡ emission_share(x,s) для s>0
                assert_eq!((x / s > 0.02), (wr_dict::emission_share(x, s) > 0.02));
                assert_eq!((x / s > 0.05), (wr_dict::emission_share(x, s) > 0.05));
            }
        }
        // гілка s<=0: emission_share*100 == 0.0 (як старий else 0.0)
        assert_eq!(wr_dict::emission_share(5.0, 0.0) * 100.0, 0.0);
    }
}
