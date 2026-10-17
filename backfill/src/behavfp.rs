//! behavfp — поведінковий підпис замість вікна синхронності.
//!
//! Попередній детектор шукав вхід «усі за 7 днів» і не знайшов нічого: розкид входів
//! 274-628 днів. Але координація не зобов'язана бути одномоментною. Десь, що працює
//! роками, лишає РЕЖИМ, а не сплеск:
//!   • година доби й день тижня — оператор або крон мають розклад, роздріб не має;
//!   • каденція — рівні інтервали 7/30 днів між діями;
//!   • ОДНАКОВА СУМА реальних грошей — стейбл-переказ на ті самі $50 000; у стейблах
//!     сума і є доларами, конвертувати нічого не треба;
//!   • спільний фандер у межах 2 хопів — звідки взявся газ і перші гроші.
//!
//! Жоден із цих сигналів окремо нічого не варт: пів-ринку торгує ввечері й переказує
//! круглі $10 000. Тому кожен зважується РІДКІСТЮ (сума чи фандер, спільні для 2-8
//! гаманців, важать; спільні для 200 — ні), а ребро вимагає ТВЕРДОГО сигналу (фандер
//! або сигнатурна сума) плюс збіг режиму. І все це проганяється через нуль-тест:
//! підписи тасуються між гаманцями, і якщо випадок дає такі самі групи — знахідки нема.
//!
//! Args: ROOT. Env: TOPH(300) MIN_ASSETS(3) CONC(8) MIN_SCORE(5) NULL_ITERS(10)
//! Вихід: dataset/backfill/behavfp.json + behavfp-report.md

use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;

const BS: &str = "https://eth.blockscout.com";

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

async fn get_json(c: &reqwest::Client, url: &str) -> Option<Value> {
    for attempt in 0..5u64 {
        if let Ok(r) = c.get(url).timeout(Duration::from_secs(30)).send().await {
            let st = r.status().as_u16();
            if st != 429 && st < 500 {
                if let Ok(v) = r.json::<Value>().await {
                    return Some(v);
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(700 * (attempt + 1))).await;
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

struct Uf(Vec<usize>);
impl Uf {
    fn new(n: usize) -> Self {
        Uf((0..n).collect())
    }
    fn find(&mut self, mut x: usize) -> usize {
        while self.0[x] != x {
            self.0[x] = self.0[self.0[x]];
            x = self.0[x];
        }
        x
    }
    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.0[ra] = rb;
        }
    }
}

#[derive(Clone, Default)]
struct Fp {
    hour: [f64; 24],
    dow: [f64; 7],
    /// суми стейблів у центах → скільки разів трапилась у цього гаманця
    amounts: HashMap<u64, u32>,
    /// частка інтервалів, близьких до 7 і до 30 днів
    p7: f64,
    p30: f64,
    funder: String,
    events: usize,
    stable_usd: f64,
}

fn cos(a: &[f64], b: &[f64]) -> f64 {
    let (mut d, mut na, mut nb) = (0.0, 0.0, 0.0);
    for i in 0..a.len() {
        d += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    if na <= 0.0 || nb <= 0.0 { 0.0 } else { d / (na.sqrt() * nb.sqrt()) }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let toph: usize = envn("TOPH", 300);
    let min_assets: usize = envn("MIN_ASSETS", 3);
    let conc: usize = envn("CONC", 8);
    let min_score: f64 = envn("MIN_SCORE", 5.0);
    let null_iters: usize = envn("NULL_ITERS", 10);
    let dir = format!("{root}/dataset/backfill");
    let keys: Vec<String> = envv(&root, "BACKFILL_ETHERSCAN_KEYS")
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    assert!(!keys.is_empty(), "BACKFILL_ETHERSCAN_KEYS порожній");

    let src: Value =
        serde_json::from_str(&fs::read_to_string(format!("{dir}/emission-buyback.json")).unwrap())
            .unwrap();
    let assets: Vec<Value> = src["assets"].as_array().cloned().unwrap_or_default();
    let client = reqwest::Client::builder()
        .user_agent("wallet-radar-behavfp/1.0")
        .build()
        .unwrap();

    // ── 1. Кандидати: приватні гаманці з >=MIN_ASSETS активів вибірки ───────
    let sem = Arc::new(Semaphore::new(conc));
    let total = assets.len();
    let assets = Arc::new(assets);
    // Режим ЕТАЛОНА: замість пошуку кандидатів беремо готовий список доведеної групи.
    // Негативний результат детектора нічого не варт, поки не показано, що на відомій
    // справжній групі він СПРАЦЬОВУЄ. Якщо ні — калібрувати треба детектор, а не світ.
    let ref_file = std::env::var("WALLETS").ok();
    let crawl_n = if ref_file.is_some() { 0 } else { total };
    let mut futs = Vec::new();
    for ai in 0..crawl_n {
        let (sem, client, assets) = (sem.clone(), client.clone(), assets.clone());
        futs.push(tokio::spawn(async move {
            let _p = sem.acquire().await.ok()?;
            let c = assets[ai]["contract"].as_str()?.to_string();
            let mut hs: Vec<String> = Vec::new();
            let mut params = String::new();
            for _ in 0..(toph.div_ceil(50) + 1) {
                if hs.len() >= toph {
                    break;
                }
                // Один збійний запит не має викидати ВЕСЬ актив: у першому прогоні `?`
                // обірвав частину активів і кандидатів лишилось 84 замість 227.
                let Some(d) = get_json(&client, &format!("{BS}/api/v2/tokens/{c}/holders{params}")).await
                else {
                    break;
                };
                let Some(items) = d["items"].as_array().cloned() else { break };
                if items.is_empty() {
                    break;
                }
                for it in &items {
                    let w = it["address"]["hash"].as_str().unwrap_or("").to_lowercase();
                    if is_burn(&w)
                        || w == c
                        || it["address"]["is_contract"].as_bool().unwrap_or(false)
                        || is_custodial(
                            it["address"]["name"].as_str().unwrap_or(""),
                            &it["address"]["metadata"]["tags"],
                        )
                    {
                        continue;
                    }
                    hs.push(w);
                    if hs.len() >= toph {
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
            Some((ai, hs))
        }));
    }
    let mut wmap: HashMap<String, Vec<usize>> = HashMap::new();
    for f in futs {
        if let Ok(Some((ai, hs))) = f.await {
            for w in hs {
                wmap.entry(w).or_default().push(ai);
            }
        }
    }
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
    let cands: Vec<String> = match &ref_file {
        Some(p) => {
            let txt = fs::read_to_string(p).expect("WALLETS файл");
            let mut v: Vec<String> = if txt.trim_start().starts_with('{') {
                // groups-tracked.json: {"A_metanet":[...], "B_stocks":[...]}
                let j: Value = serde_json::from_str(&txt).expect("json");
                let key = std::env::var("WALLETS_KEY").unwrap_or_else(|_| "A_metanet".into());
                j[&key]
                    .as_array()
                    .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_lowercase())).collect())
                    .unwrap_or_default()
            } else {
                txt.lines().map(|l| l.trim().to_lowercase()).filter(|l| l.starts_with("0x")).collect()
            };
            v.retain(|w| !is_burn(w));
            v.sort();
            v.dedup();
            v
        }
        None => {
            let mut v: Vec<String> = wmap
                .iter()
                .filter(|(w, a)| {
                    a.len() >= min_assets && known.get(*w).map(|g| g != "INFRA").unwrap_or(true)
                })
                .map(|(k, _)| k.clone())
                .collect();
            v.sort();
            v
        }
    };
    println!("[behavfp] кандидатів: {} (з {} холдерів)", cands.len(), wmap.len());

    // ── ЗАМІР ЗБАГАЧЕННЯ ПО КРОКАХ ──────────────────────────────────────────
    //
    // Питання, від якого залежить, чи жива знахідка «×103 у HUB»: збагачення дає
    // ПОВЕДІНКОВИЙ відбір чи сам факт бути топ-холдером? HUB-адреси високооборотні,
    // тож вони природно осідають у верхівках токенів. Якщо частка HUB підскакує вже
    // серед УСІХ зібраних холдерів — це ефект активності, і знахідка помирає. Якщо
    // серед холдерів вона на рівні фону, а росте лише далі — працює відбір.
    {
        let n_all = known.len().max(1) as f64;
        let mut bg: HashMap<&str, usize> = HashMap::new();
        for g in known.values() {
            *bg.entry(g.as_str()).or_insert(0) += 1;
        }
        let rate = |set: &[String], g: &str| -> (usize, f64) {
            let hit = set.iter().filter(|w| known.get(*w).map(|x| x == g).unwrap_or(false)).count();
            (hit, hit as f64 / set.len().max(1) as f64 * 100.0)
        };
        let holders: Vec<String> = wmap.keys().cloned().collect();
        println!("[behavfp] --- збагачення по кроках (фон = усе розбиття) ---");
        for g in ["HUB", "NET", "CG4"] {
            let bgr = *bg.get(g).unwrap_or(&0) as f64 / n_all * 100.0;
            let (h1, r1) = rate(&holders, g);
            let (h2, r2) = rate(&cands, g);
            println!(
                "    {g:8} фон {bgr:.4}% · усі холдери {h1} з {} = {r1:.4}% (×{:.0}) · кандидати {h2} з {} = {r2:.3}% (×{:.0})",
                holders.len(),
                if bgr > 0.0 { r1 / bgr } else { 0.0 },
                cands.len(),
                if bgr > 0.0 { r2 / bgr } else { 0.0 },
            );
        }
        if std::env::var("STAGES_ONLY").is_ok() {
            let stats: Vec<Value> = ["HUB", "NET", "CG4"]
                .iter()
                .map(|g| {
                    let bgr = *bg.get(*g).unwrap_or(&0) as f64 / n_all * 100.0;
                    let (h1, r1) = rate(&holders, g);
                    let (h2, r2) = rate(&cands, g);
                    json!({"group": g, "backgroundPct": bgr, "holdersHits": h1, "holdersPct": r1,
                           "candHits": h2, "candPct": r2, "nHolders": holders.len(), "nCands": cands.len()})
                })
                .collect();
            fs::write(
                format!("{dir}/behavfp-stages.json"),
                serde_json::to_string_pretty(&json!({"stages": stats})).unwrap(),
            )
            .ok();
            println!("[behavfp] STAGES_ONLY → {dir}/behavfp-stages.json");
            return;
        }
    }

    // ── 2. Поведінковий підпис кожного ──────────────────────────────────────
    let keys = Arc::new(keys);
    let sem2 = Arc::new(Semaphore::new(conc));
    let ctr = Arc::new(AtomicUsize::new(0));
    let ncand = cands.len();
    let mut futs = Vec::new();
    for (wi, w) in cands.iter().cloned().enumerate() {
        let (sem2, client, keys, ctr) = (sem2.clone(), client.clone(), keys.clone(), ctr.clone());
        futs.push(tokio::spawn(async move {
            let _p = sem2.acquire().await.ok()?;
            let mut fp = Fp::default();
            let mut ts: Vec<f64> = Vec::new();
            let es = |m: &str, i: usize| {
                format!(
                    "https://api.etherscan.io/v2/api?chainid=1&{m}&apikey={}",
                    keys[i % keys.len()]
                )
            };

            // нативні: перший вхідний = фандер (звідки газ)
            if let Some(d) = get_json(
                &client,
                &es(&format!("module=account&action=txlist&address={w}&page=1&offset=200&sort=asc"), wi),
            )
            .await
            {
                if let Some(a) = d["result"].as_array() {
                    for t in a {
                        ts.push(num(&t["timeStamp"]));
                    }
                    if let Some(t) = a.iter().find(|t| {
                        t["to"].as_str().unwrap_or("").to_lowercase() == w && num(&t["value"]) > 0.0
                    }) {
                        fp.funder = t["from"].as_str().unwrap_or("").to_lowercase();
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(200)).await;

            // стейбли: час І сума в доларах одночасно
            for (k, c) in [(wi + 1, wr_store::addrs::usdt()), (wi + 2, wr_store::addrs::usdc())] {
                if let Some(d) = get_json(
                    &client,
                    &es(
                        &format!("module=account&action=tokentx&contractaddress={c}&address={w}&page=1&offset=1000&sort=asc"),
                        k,
                    ),
                )
                .await
                {
                    if let Some(a) = d["result"].as_array() {
                        for t in a {
                            ts.push(num(&t["timeStamp"]));
                            let usd = num(&t["value"]) / 1e6; // wr_store::addrs::usdt()/wr_store::addrs::usdc() = 6 знаків
                            if usd >= 1000.0 && usd < 1e11 {
                                *fp.amounts.entry((usd * 100.0).round() as u64).or_insert(0) += 1;
                                fp.stable_usd += usd;
                            }
                        }
                    }
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }

            if ts.is_empty() {
                return None;
            }
            ts.sort_by(|a, b| a.partial_cmp(b).unwrap());
            fp.events = ts.len();
            for t in &ts {
                let d = (*t as i64).rem_euclid(86400) / 3600;
                fp.hour[d as usize] += 1.0;
                // 1970-01-01 — четвер, тому +4
                let dw = ((*t as i64) / 86400 + 4).rem_euclid(7);
                fp.dow[dw as usize] += 1.0;
            }
            // каденція: скільки інтервалів лягають на 7 та 30 днів (±10%)
            let (mut n7, mut n30, mut ng) = (0.0, 0.0, 0.0);
            for i in 1..ts.len() {
                let g = (ts[i] - ts[i - 1]) / 86400.0;
                if g < 0.5 {
                    continue;
                }
                ng += 1.0;
                if (g - 7.0).abs() <= 0.7 {
                    n7 += 1.0;
                }
                if (g - 30.0).abs() <= 3.0 {
                    n30 += 1.0;
                }
            }
            if ng > 0.0 {
                fp.p7 = n7 / ng;
                fp.p30 = n30 / ng;
            }
            let n = ctr.fetch_add(1, Ordering::Relaxed) + 1;
            if n % 25 == 0 {
                println!("[behavfp] підписи {n}/{ncand}");
            }
            Some((wi, fp))
        }));
    }
    let mut fps: Vec<Option<Fp>> = vec![None; cands.len()];
    for f in futs {
        if let Ok(Some((wi, fp))) = f.await {
            fps[wi] = Some(fp);
        }
    }
    let live: Vec<usize> = (0..cands.len()).filter(|i| fps[*i].is_some()).collect();
    println!("[behavfp] підписів зібрано: {}", live.len());

    // ── 3. Рідкість: сума чи фандер, спільні для півсотні, нічого не значать ─
    let mut amt_df: HashMap<u64, usize> = HashMap::new();
    let mut fund_df: HashMap<String, usize> = HashMap::new();
    for &i in &live {
        let f = fps[i].as_ref().unwrap();
        for a in f.amounts.keys() {
            *amt_df.entry(*a).or_insert(0) += 1;
        }
        if !f.funder.is_empty() {
            *fund_df.entry(f.funder.clone()).or_insert(0) += 1;
        }
    }
    // Рідкість має бути ВІДНОСНОЮ. Жорстке вікно 2..=8 при 78 гаманцях впустило $8000 і
    // $80000 — суми, які насправді трапляються в кожного десятого; вони просто впиралися
    // у стелю вікна. Тепер поріг — частка від вибірки.
    let nlive = live.len().max(1) as f64;
    let amax = (nlive * 0.06).max(2.0) as usize;
    let fmax = (nlive * 0.15).max(2.0) as usize;
    // Спільний фандер вважався твердим зв'язком — і виявився біржею: 7 із 26 членів BG1
    // «поєднував» Gate.io: Deposit Funder, ще двох — Binance 18. Кастодіальних ХОЛДЕРІВ
    // ми відсіювали, а ФАНДЕРІВ ні. Спільне походження грошей з біржі не пов'язує нікого:
    // так виведено пів-ринку.
    let labels: HashMap<String, String> = fs::read_to_string(format!("{root}/dataset/labels-cache.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v.as_object().cloned())
        .map(|o| {
            o.into_iter()
                .map(|(k, v)| (k.to_lowercase(), v["name"].as_str().unwrap_or("").to_lowercase()))
                .collect()
        })
        .unwrap_or_default();
    let exchange_funder = |f: &str| -> bool {
        match labels.get(f) {
            Some(n) if !n.is_empty() => {
                CEX_SLUGS.iter().any(|s| n.contains(s)) || n.contains("deposit") || n.contains("funder")
            }
            _ => false,
        }
    };
    let sig_amt = |a: &u64| -> bool { matches!(amt_df.get(a), Some(n) if (2..=amax).contains(n)) };
    let sig_fund = |f: &str| -> bool {
        !exchange_funder(f) && matches!(fund_df.get(f), Some(n) if (2..=fmax).contains(n))
    };
    let nex = fund_df.keys().filter(|f| exchange_funder(f)).count();
    println!("[behavfp] фандерів-бірж відкинуто: {nex} з {}", fund_df.len());
    println!("[behavfp] стеля рідкості: сума ≤{amax} гаманців, фандер ≤{fmax}");

    // ── 4. Ребра: твердий сигнал + збіг режиму ──────────────────────────────
    let score = |a: &Fp, b: &Fp| -> (f64, bool, usize, f64, f64) {
        let shared_amt = a
            .amounts
            .keys()
            .filter(|k| b.amounts.contains_key(*k) && sig_amt(k))
            .count();
        let same_fund =
            !a.funder.is_empty() && a.funder == b.funder && sig_fund(&a.funder);
        let hc = cos(&a.hour, &b.hour);
        let dc = cos(&a.dow, &b.dow);
        let cad = (a.p7 > 0.25 && b.p7 > 0.25) || (a.p30 > 0.25 && b.p30 > 0.25);
        let mut s = 0.0;
        if same_fund {
            s += 4.0;
        }
        s += 2.0 * (shared_amt.min(3) as f64);
        if hc > 0.90 {
            s += 2.0;
        }
        if dc > 0.90 {
            s += 1.0;
        }
        if cad {
            s += 1.0;
        }
        (s, same_fund, shared_amt, hc, dc)
    };

    let mut uf = Uf::new(cands.len());
    let mut edges: Vec<Value> = Vec::new();
    let mut all_scores: Vec<f64> = Vec::new();
    for ii in 0..live.len() {
        for jj in (ii + 1)..live.len() {
            let (i, j) = (live[ii], live[jj]);
            let (a, b) = (fps[i].as_ref().unwrap(), fps[j].as_ref().unwrap());
            let (s, sf, sa, hc, dc) = score(a, b);
            all_scores.push(s);
            // Твердий сигнал обов'язковий: сам по собі збіг «вечірньої години» —
            // це не зв'язок, це годинний пояс.
            if s >= min_score && (sf || sa >= 1) {
                uf.union(i, j);
                if edges.len() < 400 {
                    edges.push(json!({
                        "a": cands[i], "b": cands[j], "score": s,
                        "sameFunder": sf, "sharedAmounts": sa,
                        "hourCos": hc, "dowCos": dc
                    }));
                }
            }
        }
    }
    let mut comp: HashMap<usize, Vec<usize>> = HashMap::new();
    for &i in &live {
        let r = uf.find(i);
        comp.entry(r).or_default().push(i);
    }
    let mut groups: Vec<Vec<usize>> = comp.into_values().filter(|v| v.len() >= 3).collect();
    groups.sort_by_key(|g| std::cmp::Reverse(g.len()));
    println!("[behavfp] груп (≥3): {} · ребер: {}", groups.len(), edges.len());

    // ── 5. Нуль-тест: підписи тасуються між гаманцями ───────────────────────
    let mut seed = 0xD1B54A32D192ED03u64;
    let mut rnd = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    // Перестановка ЦІЛИХ підписів між гаманцями — беззмістовна: множина пар не
    // змінюється, граф виходить ізоморфний, і нуль щоразу дорівнює спостереженому
    // (перший прогін: 2 групи / 21 в обох). Правильний нуль руйнує зв'язок МІЖ
    // компонентами підпису: фандер від одного гаманця, суми від другого, режим від
    // третього. Маргінальні розподіли кожного сигналу зберігаються — зникає лише те,
    // що вони належали одному власнику. Саме це й перевіряємо.
    let (mut ng_mean, mut nl_max) = (0.0f64, 0usize);
    let shuffled = |rnd: &mut dyn FnMut() -> u64| -> Vec<usize> {
        let mut p: Vec<usize> = (0..live.len()).collect();
        for i in (1..p.len()).rev() {
            let j = (rnd() as usize) % (i + 1);
            p.swap(i, j);
        }
        p
    };
    for _ in 0..null_iters {
        let (pf, pa, ph) = (shuffled(&mut rnd), shuffled(&mut rnd), shuffled(&mut rnd));
        let synth: Vec<Fp> = (0..live.len())
            .map(|i| {
                let base = fps[live[ph[i]]].as_ref().unwrap();
                Fp {
                    hour: base.hour,
                    dow: base.dow,
                    p7: base.p7,
                    p30: base.p30,
                    amounts: fps[live[pa[i]]].as_ref().unwrap().amounts.clone(),
                    funder: fps[live[pf[i]]].as_ref().unwrap().funder.clone(),
                    events: base.events,
                    stable_usd: base.stable_usd,
                }
            })
            .collect();
        let mut nuf = Uf::new(cands.len());
        for ii in 0..live.len() {
            for jj in (ii + 1)..live.len() {
                let (s, sf, sa, _, _) = score(&synth[ii], &synth[jj]);
                if s >= min_score && (sf || sa >= 1) {
                    nuf.union(live[ii], live[jj]);
                }
            }
        }
        let mut nc: HashMap<usize, usize> = HashMap::new();
        for &i in &live {
            let r = nuf.find(i);
            *nc.entry(r).or_insert(0) += 1;
        }
        let sizes: Vec<usize> = nc.into_values().filter(|v| *v >= 3).collect();
        ng_mean += sizes.len() as f64;
        nl_max = nl_max.max(sizes.iter().copied().max().unwrap_or(0));
    }
    ng_mean /= null_iters.max(1) as f64;
    println!(
        "[behavfp] нуль: {ng_mean:.1} груп, найбільша до {nl_max} · спостережено {} груп, найбільша {}",
        groups.len(),
        groups.first().map(|g| g.len()).unwrap_or(0)
    );

    // ── 6. Опис груп ────────────────────────────────────────────────────────
    let mut out = Vec::new();
    for (gi, g) in groups.iter().enumerate() {
        let mut funders: HashMap<&str, usize> = HashMap::new();
        let mut amts: HashMap<u64, usize> = HashMap::new();
        let (mut stable, mut p7, mut p30) = (0.0, 0.0, 0.0);
        let mut hour = [0.0f64; 24];
        let mut abasket: HashMap<usize, usize> = HashMap::new();
        for &i in g {
            let f = fps[i].as_ref().unwrap();
            if !f.funder.is_empty() {
                *funders.entry(f.funder.as_str()).or_insert(0) += 1;
            }
            for (a, _) in f.amounts.iter().filter(|(a, _)| sig_amt(a)) {
                *amts.entry(*a).or_insert(0) += 1;
            }
            stable += f.stable_usd;
            p7 += f.p7;
            p30 += f.p30;
            for h in 0..24 {
                hour[h] += f.hour[h];
            }
            for ai in wmap.get(&cands[i]).map(|v| v.as_slice()).unwrap_or(&[]) {
                *abasket.entry(*ai).or_insert(0) += 1;
            }
        }
        let n = g.len() as f64;
        let mut fv: Vec<(&str, usize)> = funders.into_iter().filter(|x| x.1 >= 2).collect();
        fv.sort_by_key(|x| std::cmp::Reverse(x.1));
        let mut av: Vec<(u64, usize)> = amts.into_iter().filter(|x| x.1 >= 2).collect();
        av.sort_by_key(|x| std::cmp::Reverse(x.1));
        let mut bv: Vec<(usize, usize)> = abasket.into_iter().filter(|x| x.1 >= 2).collect();
        bv.sort_by_key(|x| std::cmp::Reverse(x.1));
        let peak = (0..24).max_by(|a, b| hour[*a].partial_cmp(&hour[*b]).unwrap()).unwrap_or(0);
        let hsum: f64 = hour.iter().sum::<f64>().max(1.0);
        let mut kc: HashMap<&str, usize> = HashMap::new();
        for &i in g {
            *kc.entry(known.get(&cands[i]).map(|s| s.as_str()).unwrap_or("—поза розбиттям"))
                .or_insert(0) += 1;
        }
        let mut kv: Vec<(&str, usize)> = kc.into_iter().collect();
        kv.sort_by_key(|x| std::cmp::Reverse(x.1));
        let kg: Vec<Value> =
            kv.iter().take(4).map(|(g, n)| json!({"group": g, "n": n})).collect();
        out.push(json!({
            "id": format!("BG{}", gi + 1),
            "size": g.len(),
            "members": g.iter().map(|&i| cands[i].as_str()).collect::<Vec<&str>>(),
            "stableUsdSeen": stable,
            "sharedFunders": fv.iter().take(5).map(|(f,n)| json!({"funder":f,"members":n})).collect::<Vec<Value>>(),
            "signatureAmounts": av.iter().take(8).map(|(a,n)| json!({"usd": *a as f64/100.0, "members": n})).collect::<Vec<Value>>(),
            "basket": bv.iter().take(8).map(|(ai,n)| json!({"symbol": assets[*ai]["symbol"].clone(), "sector": assets[*ai]["sector"].clone(), "members": n})).collect::<Vec<Value>>(),
            "peakHourUtc": peak,
            "peakHourShare": hour[peak]/hsum,
            "cadence7": p7/n,
            "cadence30": p30/n,
            "knownGroups": kg,
        }));
    }

    let res = json!({
        "params": {"topHolders": toph, "minAssets": min_assets, "minScore": min_score},
        "candidates": cands.len(),
        "fingerprinted": live.len(),
        "null": {"groupsMean": ng_mean, "largestMax": nl_max, "iters": null_iters},
        "observed": {"groups": groups.len(), "largest": groups.first().map(|g| g.len()).unwrap_or(0)},
        "groups": out,
        "topEdges": edges.iter().take(40).cloned().collect::<Vec<Value>>(),
    });
    fs::write(format!("{dir}/behavfp.json"), serde_json::to_string_pretty(&res).unwrap()).ok();
    fs::write(format!("{dir}/behavfp-report.md"), report(&res)).ok();
    println!("[behavfp] готово → {dir}/behavfp.json + behavfp-report.md");
}

fn report(r: &Value) -> String {
    let empty = vec![];
    let mut s = String::new();
    s.push_str("# Поведінкові групи: режим, суми, фандери\n\n");
    s.push_str(&format!(
        "Кандидатів {} · підписів зібрано {} · груп {} (найбільша {})\n\n",
        r["candidates"], r["fingerprinted"], r["observed"]["groups"], r["observed"]["largest"]
    ));
    s.push_str(&format!(
        "**Нуль-тест** ({} тасувань підписів між гаманцями): випадок дає {:.1} груп, найбільша до {}. \
         Знахідкою є лише перевищення цього фону.\n\n",
        r["null"]["iters"],
        num(&r["null"]["groupsMean"]),
        r["null"]["largestMax"]
    ));
    for g in r["groups"].as_array().unwrap_or(&empty) {
        s.push_str(&format!(
            "## {} — {} гаманців · пік {}:00 UTC ({:.0}% активності) · каденція 7д {:.0}% / 30д {:.0}%\n\n",
            g["id"].as_str().unwrap_or(""),
            g["size"],
            g["peakHourUtc"],
            num(&g["peakHourShare"]) * 100.0,
            num(&g["cadence7"]) * 100.0,
            num(&g["cadence30"]) * 100.0,
        ));
        s.push_str(&format!("Стейблів пройшло: ${:.1}М\n\n", num(&g["stableUsdSeen"]) / 1e6));
        let f = g["sharedFunders"].as_array().unwrap_or(&empty);
        if !f.is_empty() {
            s.push_str("Спільні фандери: ");
            s.push_str(
                &f.iter()
                    .map(|x| format!("{}… ×{}", &x["funder"].as_str().unwrap_or("")[..12], x["members"]))
                    .collect::<Vec<String>>()
                    .join(", "),
            );
            s.push_str("\n\n");
        }
        let a = g["signatureAmounts"].as_array().unwrap_or(&empty);
        if !a.is_empty() {
            s.push_str("Сигнатурні суми: ");
            s.push_str(
                &a.iter()
                    .map(|x| format!("${:.0} ×{}", num(&x["usd"]), x["members"]))
                    .collect::<Vec<String>>()
                    .join(", "),
            );
            s.push_str("\n\n");
        }
        let b = g["basket"].as_array().unwrap_or(&empty);
        if !b.is_empty() {
            s.push_str("Кошик: ");
            s.push_str(
                &b.iter()
                    .map(|x| {
                        format!(
                            "{} ({}) ×{}",
                            x["symbol"].as_str().unwrap_or(""),
                            x["sector"].as_str().unwrap_or("?"),
                            x["members"]
                        )
                    })
                    .collect::<Vec<String>>()
                    .join(", "),
            );
            s.push_str("\n\n");
        }
    }
    s
}
