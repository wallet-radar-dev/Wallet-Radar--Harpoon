//! livewatch — живий монітор мереж: що роблять адреси груп просто зараз.
//!
//! Наявні монітори опитують Etherscan ПОАДРЕСНО (relay_monitor — по реле, group_monitor —
//! по ядрах). Для 13 333 адрес такий підхід неможливий: це тисячі запитів на цикл і
//! миттєве вигоряння квоти. Тому тут інший принцип — читаємо БЛОКИ. Один запит покриває
//! всі адреси одразу, скільки б їх не було: ~7200 блоків на добу проти квоти 100 000.
//!
//! Що бін вміє сказати про кожну подію:
//!   * ЩО сталося — депозит на біржу, виведення, внутрішній переказ, роздача;
//!   * ХТО це зробив — роль адреси береться з накопиченої бази (тримач, транзит, казна,
//!     підписант), а не вигадується наново;
//!   * КУДИ саме — назва майданчика, бо ролі бірж у цієї мережі різні: з Binance і
//!     Coinbase переважно виводять, а на crypto.com і gate.io заводять;
//!   * ПОЧІМ — ціна активу в момент дії, з публічного API біржі (ключі не потрібні:
//!     приватна частина API біржі стосується лише власника ключа й про чужі гаманці
//!     не знає нічого).
//!
//! Стан: deploy/livewatch.state.json (останній оброблений блок). Без стану бін почав би
//! з нуля й засипав канал історією.
//!
//! Args: ROOT. Env: CHAIN(1) MIN_USD(100000) MAX_BLOCKS(120) DRY(1 — не слати в Telegram)
//!       QUIET(1 — без ринкового контексту) LOOP_SECS(0 — один прохід)
use backfill::known::{self, pat};
use backfill::patterns::PatternDb;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufReader, Write};

/// topic0 події Transfer(address,address,uint256) — однаковий для всіх ERC-20.
const TRANSFER_TOPIC: &str = "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef";

fn envn<T: std::str::FromStr>(k: &str, d: T) -> T {
    std::env::var(k).ok().and_then(|s| s.parse().ok()).unwrap_or(d)
}

fn envv(root: &str, k: &str) -> String {
    std::fs::read_to_string(format!("{root}/.env"))
        .unwrap_or_default()
        .lines()
        .find(|l| l.starts_with(&format!("{k}=")))
        .map(|l| l.splitn(2, '=').nth(1).unwrap_or("").trim().to_string())
        .unwrap_or_default()
}

/// Звична роль майданчика в цій мережі — перевірено по семи роках поспіль.
///
/// Розподіл ролей виявився не випадковим, а закріпленим: з Binance, Gemini, Bitfinex і
/// Bitstamp мережа лише виводить (жодного року з іншим знаком), а на crypto.com і gate.io
/// лише заводить. Тому дія проти цієї ролі — сама по собі новина: вона означає або зміну
/// схеми, або те, що діє хтось інший.
fn usual_role(exchange: &str) -> Option<&'static str> {
    match exchange {
        "binance" | "gemini" | "bitfinex" | "bitstamp" | "coinbase" | "kraken" => Some("виведення"),
        "crypto.com" | "gate.io" => Some("заведення"),
        _ => None, // okx та інші поводяться змішано — тлумачити нема чого
    }
}

/// Подія, яку варто показати людині.
struct Event {
    kind: &'static str,
    addr: String,
    counterparty: String,
    exchange: Option<String>,
    sym: String,
    amount: f64,
    usd: f64,
    hash: String,
    block: u64,
}

/// Перші 12 знаків адреси: досить, щоб упізнати, і не ламає рядок.
fn short(a: &str) -> &str {
    if a.len() >= 12 {
        &a[..12]
    } else {
        a
    }
}

/// Згортає перекази однієї транзакції для однієї адреси в одну подію.
///
/// Обмін через роутер приходить кількома переказами: адреса віддала один актив і
/// тим самим хешем отримала інший. У стрічці це виглядало як дві-три окремі новини
/// про ту саму дію — і завищувало лічильник подій. Тепер це один рядок «ОБМІН».
fn collapse_by_tx(events: Vec<Event>) -> Vec<Event> {
    let mut order: Vec<(String, String)> = Vec::new();
    let mut groups: HashMap<(String, String), Vec<Event>> = HashMap::new();
    for e in events {
        let k = (e.hash.clone(), e.addr.clone());
        if !groups.contains_key(&k) {
            order.push(k.clone());
        }
        groups.entry(k).or_default().push(e);
    }
    let mut out = Vec::new();
    for k in order {
        let Some(mut g) = groups.remove(&k) else { continue };
        // найбільша нога представляє дію: дрібні решти того ж обміну не новина
        g.sort_by(|a, b| b.usd.partial_cmp(&a.usd).unwrap());
        let si = g.iter().position(|e| matches!(e.kind, "ВІДПРАВЛЕННЯ" | "ДЕПОЗИТ НА БІРЖУ"));
        let ri = g.iter().position(|e| matches!(e.kind, "НАДХОДЖЕННЯ" | "ВИВЕДЕННЯ З БІРЖІ"));
        if let (Some(si), Some(ri)) = (si, ri) {
            if g[si].sym != g[ri].sym {
                let (rsym, ramt) = (g[ri].sym.clone(), g[ri].amount);
                let mut e = g.swap_remove(si);
                e.kind = "ОБМІН";
                e.sym = format!("{} → {:.2} {}", e.sym, ramt, rsym);
                out.push(e);
                continue;
            }
        }
        out.push(g.swap_remove(0));
    }
    out
}

/// Ціна з публічного API біржі. Ключ не потрібен — це ринкові дані, вони відкриті.
async fn spot_price(client: &reqwest::Client, sym: &str) -> Option<f64> {
    let pair = format!("{}USDT", sym.to_uppercase());
    let url = format!("https://api.binance.com/api/v3/ticker/price?symbol={pair}");
    let v: Value = client.get(&url).send().await.ok()?.json().await.ok()?;
    v["price"].as_str()?.parse().ok()
}

/// Зміна ціни за останню годину — контекст, у якому сталася дія.
async fn hour_change(client: &reqwest::Client, sym: &str) -> Option<f64> {
    let pair = format!("{}USDT", sym.to_uppercase());
    let url = format!("https://api.binance.com/api/v3/klines?symbol={pair}&interval=1h&limit=2");
    let v: Value = client.get(&url).send().await.ok()?.json().await.ok()?;
    let arr = v.as_array()?;
    let last = arr.last()?;
    let open: f64 = last[1].as_str()?.parse().ok()?;
    let close: f64 = last[4].as_str()?.parse().ok()?;
    if open > 0.0 {
        Some(100.0 * (close - open) / open)
    } else {
        None
    }
}

async fn telegram(client: &reqwest::Client, token: &str, chat: &str, text: &str) {
    if token.is_empty() || chat.is_empty() {
        eprintln!("[livewatch] Telegram не налаштовано — друкую лише в консоль");
        return;
    }
    let url = format!("https://api.telegram.org/bot{token}/sendMessage");
    let _ = client
        .post(&url)
        .json(&json!({"chat_id": chat, "text": text, "parse_mode": "HTML",
                      "disable_web_page_preview": true}))
        .send()
        .await;
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let bf = format!("{root}/dataset/backfill");
    let chain: u32 = envn("CHAIN", 1);
    let min_usd: f64 = envn("MIN_USD", 100_000.0);
    let max_blocks: u64 = envn("MAX_BLOCKS", 120);
    let dry = std::env::var("DRY").is_ok();
    let quiet = std::env::var("QUIET").is_ok();
    let loop_secs: u64 = envn("LOOP_SECS", 0);
    let skip_tokens = std::env::var("SKIP_TOKENS").is_ok();

    let keys: Vec<String> = envv(&root, "BACKFILL_ETHERSCAN_KEYS")
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    assert!(!keys.is_empty(), "потрібні ключі Etherscan");
    let tg_token = envv(&root, "TELEGRAM_BOT_TOKEN");
    let tg_chat = envv(&root, "TELEGRAM_CHAT_ID");

    // ── кого стежимо і що про них уже відомо ──────────────────────────────────
    let db = PatternDb::load(&bf);
    let mut watched: HashSet<String> = HashSet::new();
    let mut role: HashMap<String, String> = HashMap::new();
    let mut cex: HashSet<String> = HashSet::new();

    // назва майданчика для клієнтських депозитних адрес — з бази, куди її поклав flowdir.
    // Мітки покривають лише гарячі гаманці бірж (сотні адрес), а більшість депозитів іде
    // на клієнтські адреси, яких ніхто не мітить: без цього в сповіщенні було б безлике
    // «депозит на біржу», хоча саме назва й несе зміст.
    let mut cex_name: HashMap<String, String> = HashMap::new();
    for (a, recs) in &db.map {
        if let Some(r) = recs.iter().find(|r| r.pat == "CEX-DEPOSIT-ADDR") {
            if let Some(n) = r.ev.get("exchange").and_then(|v| v.as_str()) {
                cex_name.insert(a.clone(), n.to_string());
            }
        }
    }
    eprintln!("[livewatch] депозитних адрес із назвою майданчика: {}", cex_name.len());

    for (a, recs) in &db.map {
        let has = |p: &str| recs.iter().any(|r| r.pat == p);
        if has(pat::CEX_LABELED) || has("CEX-DEPOSIT-ADDR") {
            cex.insert(a.clone());
            continue; // біржа не є об'єктом стеження
        }
        // Контракт у списку стеження дає технічний шум: роутер пропускає крізь себе
        // чужі кошти, і в стрічці це виглядає як дія учасника — одна й та сама сума
        // одночасно «відправлена» і «отримана». Виняток — мультипідписні казни: там
        // за транзакцією стоїть рішення підписантів, а не виконання чужого коду.
        if has(pat::CONTRACT) && !has("SAFE") {
            continue;
        }
        if has("GRP-6864") || has("GRP-NEW") {
            watched.insert(a.clone());
            // роль беремо з бази: вона накопичена десятком аналізів і краща за здогад
            let r = if has("SAFE") {
                "казна-мультипідпис"
            } else if has("MULTI-ASSET-WHALE") {
                "кит"
            } else if has(pat::TREASURY) || has("SHARED-FUNDER") {
                "казна"
            } else if has(pat::DISTRIBUTOR) {
                "роздавач"
            } else if has("SAFE-OWNER") {
                "підписант"
            } else if has(pat::MAKER) {
                "мейкер"
            } else if has(pat::HOLDER) {
                "тримач"
            } else {
                "член мережі"
            };
            role.insert(a.clone(), r.to_string());
        }
    }

    // мітки гарячих гаманців доповнюють картину: їх мало, але вони найточніші
    if let Ok(f) = File::open(format!("{bf}/labels-cache.json")) {
        if let Ok(m) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(f)) {
            for (a, v) in m {
                let mut s = v["name"].as_str().unwrap_or("").to_string();
                if let Some(arr) = v["labels"].as_array() {
                    for x in arr {
                        if let Some(t) = x.as_str() {
                            s.push(' ');
                            s.push_str(t)
                        }
                    }
                }
                let sl = s.to_lowercase();
                if let Some(w) = known::CEX_WORDS
                    .iter()
                    .find(|w| !matches!(**w, "deposit" | "otc" | "exchange") && sl.contains(**w))
                {
                    cex_name.insert(a.to_lowercase(), (*w).to_string());
                }
            }
        }
    }
    eprintln!(
        "[livewatch] стежимо за {} адресами · біржових {} · названих майданчиків {}",
        watched.len(),
        cex.len(),
        cex_name.len()
    );

    // символ, decimals і поточна ціна токена: без них сума переказу нічого не важить
    let mut tok_meta: HashMap<String, (String, i32)> = HashMap::new();
    let mut tok_price: HashMap<String, f64> = HashMap::new();
    if let Ok(f) = File::open(format!("{bf}/token-supply.json")) {
        if let Ok(m) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(f)) {
            for (c, v) in m {
                let d = v["decimals"].as_i64().unwrap_or(18) as i32;
                let sym = v["sym"].as_str().or(v["symbol"].as_str()).unwrap_or("").to_string();
                tok_meta.insert(c.to_lowercase(), (sym, d));
            }
        }
    }
    if let Ok(f) = File::open(format!("{bf}/token-prices.json")) {
        if let Ok(v) = serde_json::from_reader::<_, Value>(BufReader::new(f)) {
            if let Some(o) = v["tokens"].as_object() {
                for (c, x) in o {
                    if let Some(p) = x["usd"].as_f64() {
                        tok_price.insert(c.to_lowercase(), p);
                    }
                    if let Some(sy) = x["sym"].as_str() {
                        tok_meta.entry(c.to_lowercase())
                            .or_insert_with(|| (sy.to_string(), 18));
                    }
                }
            }
        }
    }
    eprintln!("[livewatch] токенів із метаданими {} · із ціною {}", tok_meta.len(), tok_price.len());

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap();
    let state_path = format!("{root}/deploy/livewatch.state.json");
    let mut kc = 0usize;

    loop {
        // ── де ми зупинились ──────────────────────────────────────────────────
        let mut last_block: u64 = std::fs::read_to_string(&state_path)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .and_then(|v| v["lastBlock"].as_u64())
            .unwrap_or(0);

        // поточна висота
        let url = format!(
            "https://api.etherscan.io/v2/api?chainid={chain}&module=proxy&action=eth_blockNumber&apikey={}",
            keys[kc % keys.len()]
        );
        kc += 1;
        let head: u64 = match client.get(&url).send().await {
            Ok(r) => match r.json::<Value>().await {
                Ok(v) => v["result"]
                    .as_str()
                    .and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok())
                    .unwrap_or(0),
                Err(_) => 0,
            },
            Err(_) => 0,
        };
        if head == 0 {
            eprintln!("[livewatch] не вдалось дізнатись висоту ланцюга");
            if loop_secs == 0 {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_secs(loop_secs)).await;
            continue;
        }
        if last_block == 0 {
            // Перший запуск: беремо невелике вікно назад, а не всю історію — інакше
            // перше ж сповіщення було б лавиною подій за роки.
            last_block = head.saturating_sub(50);
            eprintln!("[livewatch] перший запуск, починаю з блоку {last_block}");
        }
        // Монітор має сенс, лише поки показує теперішнє. За добу ланцюг дає ~7200
        // блоків — рівно стільки ж встигає обробити цикл, тож накопичене відставання
        // (зупинка, збій, пауза) не розсмоктується саме: воно лишається назавжди.
        // Чесніше стрибнути до голови й сказати, скільки блоків пропущено.
        let max_lag: u64 = envn("MAX_LAG", 1200);
        if head.saturating_sub(last_block) > max_lag {
            let target = head - max_lag;
            eprintln!(
                "[livewatch] відставання {} блоків — пропускаю {} і йду до голови",
                head - last_block,
                target - last_block
            );
            last_block = target;
        }
        let upto = head.min(last_block + max_blocks);
        if upto <= last_block {
            eprintln!("[livewatch] нових блоків немає (висота {head})");
            if loop_secs == 0 {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_secs(loop_secs)).await;
            continue;
        }
        eprintln!("[livewatch] обробляю блоки {}..{upto} (висота {head})", last_block + 1);

        // ── прохід по блоках ──────────────────────────────────────────────────
        let mut events: Vec<Event> = Vec::new();
        let eth_price = spot_price(&client, "ETH").await.unwrap_or(0.0);

        for b in (last_block + 1)..=upto {
            let url = format!(
                "https://api.etherscan.io/v2/api?chainid={chain}&module=proxy&action=eth_getBlockByNumber&tag=0x{:x}&boolean=true&apikey={}",
                b,
                keys[kc % keys.len()]
            );
            kc += 1;
            let Ok(resp) = client.get(&url).send().await else { continue };
            let Ok(v) = resp.json::<Value>().await else { continue };
            let Some(txs) = v["result"]["transactions"].as_array() else { continue };

            for t in txs {
                let from = t["from"].as_str().unwrap_or("").to_lowercase();
                let to = t["to"].as_str().unwrap_or("").to_lowercase();
                if from.is_empty() || to.is_empty() {
                    continue;
                }
                let ours_from = watched.contains(&from);
                let ours_to = watched.contains(&to);
                if !ours_from && !ours_to {
                    continue;
                }
                let wei = t["value"]
                    .as_str()
                    .and_then(|s| u128::from_str_radix(s.trim_start_matches("0x"), 16).ok())
                    .unwrap_or(0);
                let eth = wei as f64 / 1e18;
                let usd = eth * eth_price;
                if usd < min_usd {
                    continue;
                }

                let (kind, addr, counterparty, exchange) = if ours_from && cex.contains(&to) {
                    ("ДЕПОЗИТ НА БІРЖУ", from.clone(), to.clone(), cex_name.get(&to).cloned())
                } else if ours_to && cex.contains(&from) {
                    ("ВИВЕДЕННЯ З БІРЖІ", to.clone(), from.clone(), cex_name.get(&from).cloned())
                } else if ours_from && ours_to {
                    ("ВНУТРІШНІЙ ПЕРЕКАЗ", from.clone(), to.clone(), None)
                } else if ours_from {
                    ("ВІДПРАВЛЕННЯ", from.clone(), to.clone(), None)
                } else {
                    ("НАДХОДЖЕННЯ", to.clone(), from.clone(), None)
                };

                events.push(Event {
                    kind,
                    addr,
                    counterparty,
                    exchange,
                    sym: "ETH".into(),
                    amount: eth,
                    usd,
                    hash: t["hash"].as_str().unwrap_or("").to_string(),
                    block: b,
                });
            }
            // ── токенні перекази того самого блоку ────────────────────────────
            //
            // Без цього монітор сліпий до більшої частини дій: мережа тримає позиції
            // в токенах, а ETH здебільшого возить як паливо й транзит. Фільтрувати
            // логи за нашими адресами на боці API неможливо (їх 13 тисяч, а topic
            // приймає одиниці), тож беремо всі Transfer блоку й звіряємо локально.
            if !skip_tokens {
                let url = format!(
                    "https://api.etherscan.io/v2/api?chainid={chain}&module=logs&action=getLogs&fromBlock={b}&toBlock={b}&topic0={TRANSFER_TOPIC}&apikey={}",
                    keys[kc % keys.len()]
                );
                kc += 1;
                if let Ok(resp) = client.get(&url).send().await {
                    if let Ok(v) = resp.json::<Value>().await {
                        if let Some(logs) = v["result"].as_array() {
                            for lg in logs {
                                let topics = lg["topics"].as_array().map(|x| x.as_slice()).unwrap_or(&[]);
                                if topics.len() < 3 {
                                    continue; // NFT-перекази мають 4 теми, нам потрібні ERC-20
                                }
                                let addr_of = |t: &Value| -> String {
                                    let s = t.as_str().unwrap_or("");
                                    if s.len() >= 42 {
                                        format!("0x{}", &s[s.len() - 40..]).to_lowercase()
                                    } else {
                                        String::new()
                                    }
                                };
                                let from = addr_of(&topics[1]);
                                let to = addr_of(&topics[2]);
                                let ours_from = watched.contains(&from);
                                let ours_to = watched.contains(&to);
                                if !ours_from && !ours_to {
                                    continue;
                                }
                                let token = lg["address"].as_str().unwrap_or("").to_lowercase();
                                let Some((sym, dec)) = tok_meta.get(&token) else { continue };
                                let raw = lg["data"].as_str().unwrap_or("0x0");
                                let amt = u128::from_str_radix(raw.trim_start_matches("0x"), 16)
                                    .unwrap_or(0) as f64
                                    / 10f64.powi(*dec);
                                let usd = amt * tok_price.get(&token).copied().unwrap_or(0.0);
                                if usd < min_usd {
                                    continue;
                                }
                                let (kind, addr, counterparty, exchange) = if ours_from && cex.contains(&to) {
                                    ("ДЕПОЗИТ НА БІРЖУ", from.clone(), to.clone(), cex_name.get(&to).cloned())
                                } else if ours_to && cex.contains(&from) {
                                    ("ВИВЕДЕННЯ З БІРЖІ", to.clone(), from.clone(), cex_name.get(&from).cloned())
                                } else if ours_from && ours_to {
                                    ("ВНУТРІШНІЙ ПЕРЕКАЗ", from.clone(), to.clone(), None)
                                } else if ours_from {
                                    ("ВІДПРАВЛЕННЯ", from.clone(), to.clone(), None)
                                } else {
                                    ("НАДХОДЖЕННЯ", to.clone(), from.clone(), None)
                                };
                                events.push(Event {
                                    kind,
                                    addr,
                                    counterparty,
                                    exchange,
                                    sym: sym.clone(),
                                    amount: amt,
                                    usd,
                                    hash: lg["transactionHash"].as_str().unwrap_or("").to_string(),
                                    block: b,
                                });
                            }
                        }
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(120)).await;
            }
            tokio::time::sleep(std::time::Duration::from_millis(120)).await;
        }

        // ── сповіщення ────────────────────────────────────────────────────────
        // згортаємо ДО підрахунку: інакше лічильник рахував би ноги обміну як події
        let mut events = collapse_by_tx(events);
        eprintln!("[livewatch] подій ≥${:.0}: {}", min_usd, events.len());
        // Розподіл за сумою: поріг сповіщень має вибиратись із того, скільки подій
        // він реально лишить у стрічці, а не з відчуття.
        for t in [50_000.0, 100_000.0, 250_000.0, 1_000_000.0, 5_000_000.0] {
            if t >= min_usd {
                let n = events.iter().filter(|e| e.usd >= t).count();
                eprintln!("[livewatch]   з них ≥${:>9.0}: {}", t, n);
            }
        }
        if !events.is_empty() {
            let chg = if quiet { None } else { hour_change(&client, "ETH").await };
            let mut msg = String::new();
            msg.push_str(&format!(
                "<b>Активність мережі</b> · блоки {}..{}\n",
                last_block + 1,
                upto
            ));
            if let Some(c) = chg {
                msg.push_str(&format!("ETH ${:.0} · за годину {:+.2}%\n", eth_price, c));
            }
            msg.push('\n');

            // Подію, яку не можна відкрити й перевірити, немає сенсу слати.
            let tx_base = match chain {
                1 => "https://etherscan.io/tx/",
                42161 => "https://arbiscan.io/tx/",
                137 => "https://polygonscan.com/tx/",
                8453 => "https://basescan.org/tx/",
                _ => "https://blockscan.com/tx/",
            };
            events.sort_by(|a, b| b.usd.partial_cmp(&a.usd).unwrap());
            for e in events.iter().take(15) {
                let r = role.get(&e.addr).cloned().unwrap_or_else(|| "?".into());
                let place = e
                    .exchange
                    .as_ref()
                    .map(|x| format!(" ({x})"))
                    .unwrap_or_default();
                // Чи вкладається дія у звичну роль майданчика. Позначаємо лише розбіжність:
                // підтвердження очікуваного не є новиною і лише засмічувало б канал.
                let odd = match (&e.exchange, e.kind) {
                    (Some(x), "ДЕПОЗИТ НА БІРЖУ") if usual_role(x) == Some("виведення") => {
                        " ⚠ незвично: сюди зазвичай не заводять"
                    }
                    (Some(x), "ВИВЕДЕННЯ З БІРЖІ") if usual_role(x) == Some("заведення") => {
                        " ⚠ незвично: звідси зазвичай не виводять"
                    }
                    _ => "",
                };
                msg.push_str(&format!(
                    "<b>{}</b>{}{}\n{:.2} {} · ${:.0}\n<code>{}</code> — {} → <code>{}</code>\n{}{}\n\n",
                    e.kind,
                    place,
                    odd,
                    e.amount,
                    e.sym,
                    e.usd,
                    short(&e.addr),
                    r,
                    short(&e.counterparty),
                    tx_base,
                    e.hash
                ));
            }
            if events.len() > 15 {
                msg.push_str(&format!("…і ще {} подій\n", events.len() - 15));
            }

            println!("{}", msg.replace("<b>", "").replace("</b>", "")
                .replace("<code>", "").replace("</code>", ""));
            if !dry {
                telegram(&client, &tg_token, &tg_chat, &msg).await;
            }
        }

        // стан пишемо ЛИШЕ після успішної обробки: інакше збій пропустив би блоки назавжди
        let _ = File::create(&state_path).and_then(|mut f| {
            f.write_all(
                serde_json::to_string(&json!({"lastBlock": upto, "head": head}))
                    .unwrap()
                    .as_bytes(),
            )
        });

        if loop_secs == 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_secs(loop_secs)).await;
    }
}
