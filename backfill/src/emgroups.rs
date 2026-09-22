//! emgroups — скоординовані групи капіталу серед активів із викупленою емісією.
//!
//! Вхід: `emission-buyback.json` (активи, що пройшли поріг EOA-частки). Для кожного
//! добираємо топ холдерів і лишаємо ЛИШЕ приватні гаманці: спалене, кастодія бірж і
//! контракти (стейкінг, вестинг, мости, пули) — це не чиясь позиція, і група з них
//! була б артефактом.
//!
//! Координацію не оголошуємо за фактом спільного активу — два кити в USDC нічого не
//! означають. Рахуємо СПІЛЬНУ РІДКІСТЬ: ваговий внесок активу = ln(усього холдерів /
//! глибина топу), тож сусідство у верхівці токена з мільйоном власників важить набагато
//! більше, ніж у токена з півсотнею. Пара гаманців стає ребром лише коли перетин ≥
//! MIN_ASSETS активів І сумарна вага ≥ MIN_W; далі union-find збирає компоненти.
//!
//! Два незалежні перехресні тести на кожну знайдену групу:
//!   1. збіг із вже відомим розбиттям (`group-membership.json`) — якщо група співпала
//!      з раніше доведеною, це не нова знахідка, а те саме іншим шляхом;
//!   2. СИНХРОННИЙ ВХІД — через Etherscan беремо перший вхідний переказ кожного члена
//!      по спільному активу. Ідентичні портфелі бувають випадково, вхід в одне вікно —
//!      майже ні. Без цього тесту «група» лишається кореляцією, а не координацією.
//!
//! Args: ROOT. Env: TOPH(300) MIN_ASSETS(3) MIN_W(6) CONC(6) MAX_CLUSTERS(25) SYNC_DAYS(7)
//! Вихід: dataset/backfill/emgroups.json + emgroups-report.md

use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
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
    // Перелік спалених адрес — у базі, тег `burn`. Їхній баланс вийшов з обігу
    // і в знаменник емісії не входить.
    wr_store::snapshot().wallet_has_tag("eth", a, "burn")
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
        match c.get(url).timeout(Duration::from_secs(30)).send().await {
            Ok(r) => {
                let st = r.status().as_u16();
                if st == 429 || st >= 500 {
                    tokio::time::sleep(Duration::from_millis(1200 * (attempt + 1))).await;
                    continue;
                }
                if let Ok(v) = r.json::<Value>().await {
                    return Some(v);
                }
            }
            Err(_) => {}
        }
        tokio::time::sleep(Duration::from_millis(500 * (attempt + 1))).await;
    }
    None
}

fn qs(next: &Value) -> String {
    let Some(o) = next.as_object() else { return String::new() };
    let parts: Vec<String> = o
        .iter()
        .map(|(k, v)| {
            let s = match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            format!("{k}={s}")
        })
        .collect();
    if parts.is_empty() { String::new() } else { format!("?{}", parts.join("&")) }
}

struct Uf(Vec<usize>);
impl Uf {
    fn new(n: usize) -> Self {
        Uf((0..n).collect())
    }
    fn find(&mut self, mut x: usize) -> usize {
        while self.0[x] != x {
            self.0[x] = self.0[self.0[x]]; // path halving
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

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let toph: usize = envn("TOPH", 300);
    let min_assets: usize = envn("MIN_ASSETS", 3);
    let min_w: f64 = envn("MIN_W", 6.0);
    let conc: usize = envn("CONC", 6);
    let max_clusters: usize = envn("MAX_CLUSTERS", 25);
    let sync_days: f64 = envn("SYNC_DAYS", 7.0);
    let dir = format!("{root}/dataset/backfill");

    let src: Value = serde_json::from_str(
        &fs::read_to_string(format!("{dir}/emission-buyback.json")).expect("emission-buyback.json"),
    )
    .expect("json");
    let assets: Vec<Value> = src["assets"].as_array().cloned().unwrap_or_default();
    println!("[emgroups] активів із викупленою емісією: {}", assets.len());

    let client = reqwest::Client::builder()
        .user_agent("wallet-radar-emgroups/1.0")
        .build()
        .expect("client");

    // ── 1. Приватні холдери кожного активу ──────────────────────────────────
    let sem = Arc::new(Semaphore::new(conc));
    let done = Arc::new(AtomicUsize::new(0));
    let total = assets.len();
    let assets = Arc::new(assets);
    let mut futs = Vec::new();
    for ai in 0..total {
        let (sem, done, client, assets) = (sem.clone(), done.clone(), client.clone(), assets.clone());
        futs.push(tokio::spawn(async move {
            let _p = sem.acquire().await.ok()?;
            let a = &assets[ai];
            let contract = a["contract"].as_str()?.to_string();
            let mut holders: Vec<(String, f64)> = Vec::new();
            let mut params = String::new();
            let mut supply_seen = 0.0f64;
            for _ in 0..(toph.div_ceil(50) + 1) {
                if holders.len() >= toph {
                    break;
                }
                let Some(d) = get_json(&client, &format!("{BS}/api/v2/tokens/{contract}/holders{params}")).await
                else {
                    break;
                };
                let Some(items) = d["items"].as_array() else { break };
                if items.is_empty() {
                    break;
                }
                for it in items {
                    let w = it["address"]["hash"].as_str().unwrap_or("").to_lowercase();
                    let v = num(&it["value"]);
                    supply_seen += v;
                    if is_burn(&w) || w == contract {
                        continue;
                    }
                    if it["address"]["is_contract"].as_bool().unwrap_or(false) {
                        continue;
                    }
                    if is_custodial(it["address"]["name"].as_str().unwrap_or(""), &it["address"]["metadata"]["tags"]) {
                        continue;
                    }
                    holders.push((w, v));
                    if holders.len() >= toph {
                        break;
                    }
                }
                let next = &d["next_page_params"];
                if next.is_null() {
                    break;
                }
                params = qs(next);
                tokio::time::sleep(Duration::from_millis(120)).await;
            }
            let n = done.fetch_add(1, Ordering::Relaxed) + 1;
            if n % 20 == 0 {
                println!("[emgroups] холдери {n}/{total}");
            }
            let _ = supply_seen;
            Some((ai, holders))
        }));
    }

    let mut per_asset: Vec<(usize, Vec<(String, f64)>)> = Vec::new();
    for f in futs {
        if let Ok(Some(v)) = f.await {
            per_asset.push(v);
        }
    }
    per_asset.sort_by_key(|x| x.0);

    // ── 2. Вага активу: наскільки ексклюзивно потрапити в його верхівку ─────
    let mut weight: HashMap<usize, f64> = HashMap::new();
    for (ai, holders) in &per_asset {
        let on_chain = num(&assets[*ai]["holdersOnChain"]).max(holders.len() as f64);
        let w = (on_chain / holders.len().max(1) as f64).ln().max(0.0);
        weight.insert(*ai, w);
    }

    // гаманець → активи, які він тримає (частка від емісії й оцінка в $)
    let mut wmap: HashMap<String, Vec<(usize, f64)>> = HashMap::new();
    for (ai, holders) in &per_asset {
        let denom: f64 = holders.iter().map(|h| h.1).sum::<f64>().max(1.0);
        let top_share = num(&assets[*ai]["topSharePct"]).max(1e-9);
        let mcap = num(&assets[*ai]["marketCap"]);
        for (w, v) in holders {
            // частка гаманця в емісії ≈ його вага серед зібраних × сукупна частка топу
            let share_pct = v / denom * top_share;
            wmap.entry(w.clone()).or_default().push((*ai, share_pct * mcap / 100.0));
        }
    }
    // Відоме розбиття читаємо ДО кластеризації: INFRA — це маршрутизатори мережі, вони
    // сидять у верхівці всього підряд і склеюють будь-які групи в один клубок. У першому
    // прогоні 42 зі 132 членів EG1 були саме INFRA — група вийшла про інфраструктуру,
    // а не про капітал.
    let known: HashMap<String, String> =
        match fs::read_to_string(format!("{dir}/group-membership.json")) {
            Ok(s) => serde_json::from_str::<Value>(&s)
                .ok()
                .and_then(|v| v["wallets"].as_object().cloned())
                .map(|o| {
                    o.into_iter()
                        .filter_map(|(k, v)| {
                            v["group"].as_str().map(|g| (k.to_lowercase(), g.to_string()))
                        })
                        .collect()
                })
                .unwrap_or_default(),
            Err(_) => HashMap::new(),
        };
    let kind: HashMap<String, String> = fs::read_to_string(format!("{dir}/wallet-kind.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<HashMap<String, String>>(&s).ok())
        .unwrap_or_default();

    let cands: Vec<String> = wmap
        .iter()
        .filter(|(w, v)| {
            v.len() >= min_assets
                && known.get(*w).map(|g| g != "INFRA").unwrap_or(true)
                && kind.get(*w).map(|k| k != "contract").unwrap_or(true)
        })
        .map(|(k, _)| k.clone())
        .collect();
    println!(
        "[emgroups] гаманців усього {} · тримають ≥{min_assets} активів: {}",
        wmap.len(),
        cands.len()
    );

    let idx: HashMap<&str, usize> =
        cands.iter().enumerate().map(|(i, w)| (w.as_str(), i)).collect();

    // ── 3. Ребра за спільною рідкістю ───────────────────────────────────────
    let mut pair: HashMap<(usize, usize), (f64, usize)> = HashMap::new();
    for (ai, holders) in &per_asset {
        let w = *weight.get(ai).unwrap_or(&0.0);
        if w <= 0.0 {
            continue;
        }
        let members: Vec<usize> =
            holders.iter().filter_map(|(h, _)| idx.get(h.as_str()).copied()).collect();
        // Актив, у верхівці якого сидить пів-когорти, нічого не розрізняє — його
        // пари роздули б граф до одного суцільного компонента.
        if members.len() > 600 {
            println!(
                "[emgroups] {} пропущено як неселективний ({} кандидатів у топі)",
                assets[*ai]["symbol"].as_str().unwrap_or(""),
                members.len()
            );
            continue;
        }
        for i in 0..members.len() {
            for j in (i + 1)..members.len() {
                let k = (members[i].min(members[j]), members[i].max(members[j]));
                let e = pair.entry(k).or_insert((0.0, 0));
                e.0 += w;
                e.1 += 1;
            }
        }
    }

    let mut uf = Uf::new(cands.len());
    let mut edges = 0usize;
    for ((a, b), (w, n)) in &pair {
        if *n >= min_assets && *w >= min_w {
            uf.union(*a, *b);
            edges += 1;
        }
    }
    println!("[emgroups] ребер: {edges} (з {} пар-кандидатів)", pair.len());

    let mut comp: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..cands.len() {
        let r = uf.find(i);
        comp.entry(r).or_default().push(i);
    }
    let mut clusters: Vec<Vec<usize>> = comp.into_values().filter(|v| v.len() >= 3).collect();
    clusters.sort_by_key(|c| std::cmp::Reverse(c.len()));
    println!("[emgroups] груп (≥3 гаманці): {}", clusters.len());

    // ── 3б. НУЛЬ-ТЕСТ ───────────────────────────────────────────────────────
    //
    // Щільний кошик сам собою нічого не доводить: когорта, яка купувала альти одного
    // сезону, дасть перетини без жодної змови. Тому тасуємо зв'язки гаманець↔актив,
    // ЗБЕРІГАЮЧИ обидва степені (скільки активів у гаманця, скільки власників в активу),
    // і ганяємо ту саму кластеризацію. Якщо випадок дає такі самі групи — знахідки немає.
    let null_iters: usize = envn("NULL_ITERS", 10);
    let mut null_groups: Vec<usize> = Vec::new();
    let mut null_max: Vec<usize> = Vec::new();
    if null_iters > 0 {
        let mut pairs: Vec<(usize, usize)> = Vec::new(); // (wallet_idx, asset_idx)
        for (ai, holders) in &per_asset {
            if *weight.get(ai).unwrap_or(&0.0) <= 0.0 {
                continue;
            }
            let members: Vec<usize> =
                holders.iter().filter_map(|(h, _)| idx.get(h.as_str()).copied()).collect();
            if members.len() > 600 {
                continue;
            }
            for m in members {
                pairs.push((m, *ai));
            }
        }
        let mut seed = 0x9E3779B97F4A7C15u64;
        let mut rnd = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..null_iters {
            // double-edge swap: степені лишаються ті самі, структура руйнується
            let n = pairs.len();
            for _ in 0..(n * 8) {
                let (i, j) = ((rnd() as usize) % n, (rnd() as usize) % n);
                if pairs[i].1 == pairs[j].1 || pairs[i].0 == pairs[j].0 {
                    continue;
                }
                let (a1, a2) = (pairs[i].1, pairs[j].1);
                pairs[i].1 = a2;
                pairs[j].1 = a1;
            }
            let mut byasset: HashMap<usize, Vec<usize>> = HashMap::new();
            for (w, a) in &pairs {
                byasset.entry(*a).or_default().push(*w);
            }
            let mut np: HashMap<(usize, usize), (f64, usize)> = HashMap::new();
            for (a, ms) in &byasset {
                let w = *weight.get(a).unwrap_or(&0.0);
                if w <= 0.0 || ms.len() > 600 {
                    continue;
                }
                for i in 0..ms.len() {
                    for j in (i + 1)..ms.len() {
                        let k = (ms[i].min(ms[j]), ms[i].max(ms[j]));
                        let e = np.entry(k).or_insert((0.0, 0));
                        e.0 += w;
                        e.1 += 1;
                    }
                }
            }
            let mut nuf = Uf::new(cands.len());
            for ((a, b), (w, n)) in &np {
                if *n >= min_assets && *w >= min_w {
                    nuf.union(*a, *b);
                }
            }
            let mut nc: HashMap<usize, usize> = HashMap::new();
            for i in 0..cands.len() {
                let r = nuf.find(i);
                *nc.entry(r).or_insert(0) += 1;
            }
            let sizes: Vec<usize> = nc.into_values().filter(|v| *v >= 3).collect();
            null_groups.push(sizes.len());
            null_max.push(sizes.iter().copied().max().unwrap_or(0));
        }
        let avg = |v: &Vec<usize>| v.iter().sum::<usize>() as f64 / v.len().max(1) as f64;
        println!(
            "[emgroups] нуль-тест ({null_iters} перестановок): груп у середньому {:.1} (макс {}), \
             найбільша група в середньому {:.1} (макс {}) · спостережено: {} груп, найбільша {}",
            avg(&null_groups),
            null_groups.iter().max().copied().unwrap_or(0),
            avg(&null_max),
            null_max.iter().max().copied().unwrap_or(0),
            clusters.len(),
            clusters.first().map(|c| c.len()).unwrap_or(0),
        );
    }

    // ── 4. Склад груп ───────────────────────────────────────────────────────
    let mut out: Vec<Value> = Vec::new();
    for (ci, c) in clusters.iter().take(max_clusters).enumerate() {
        let members: Vec<&String> = c.iter().map(|&i| &cands[i]).collect();
        let mut asset_hits: HashMap<usize, usize> = HashMap::new();
        let mut usd = 0.0;
        for m in &members {
            for (ai, u) in &wmap[*m] {
                *asset_hits.entry(*ai).or_insert(0) += 1;
                usd += u;
            }
        }
        let mut shared: Vec<(usize, usize)> =
            asset_hits.into_iter().filter(|(_, n)| *n >= 2).collect();
        shared.sort_by_key(|x| std::cmp::Reverse(x.1));

        let mut gcount: HashMap<&str, usize> = HashMap::new();
        for m in &members {
            let g = known.get(*m).map(|s| s.as_str()).unwrap_or("—поза розбиттям");
            *gcount.entry(g).or_insert(0) += 1;
        }
        let mut gv: Vec<(&str, usize)> = gcount.into_iter().collect();
        gv.sort_by_key(|x| std::cmp::Reverse(x.1));

        out.push(json!({
            "id": format!("EG{}", ci + 1),
            "size": members.len(),
            "paperUsd": usd,
            "members": members.iter().map(|m| m.as_str()).collect::<Vec<&str>>(),
            "sharedAssets": shared.iter().take(12).map(|(ai, n)| json!({
                "symbol": assets[*ai]["symbol"].clone(),
                "contract": assets[*ai]["contract"].clone(),
                "sector": assets[*ai]["sector"].clone(),
                "membersHolding": n,
                "weight": weight.get(ai).copied().unwrap_or(0.0),
            })).collect::<Vec<Value>>(),
            "knownGroups": gv.iter().take(4).map(|(g, n)| json!({"group": g, "n": n})).collect::<Vec<Value>>(),
        }));
    }

    // ── 5. Синхронний вхід (Etherscan) ──────────────────────────────────────
    let keys: Vec<String> = envv(&root, "BACKFILL_ETHERSCAN_KEYS")
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if !keys.is_empty() {
        println!("[emgroups] перевіряю синхронність входу ({} ключів)", keys.len());
        let mut kc = 0usize;
        for g in out.iter_mut() {
            let members: Vec<String> = g["members"]
                .as_array()
                .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default();
            let shared: Vec<(String, String)> = g["sharedAssets"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .take(4)
                        .filter_map(|v| {
                            Some((
                                v["symbol"].as_str()?.to_string(),
                                v["contract"].as_str()?.to_string(),
                            ))
                        })
                        .collect()
                })
                .unwrap_or_default();
            let mut sync: Vec<Value> = Vec::new();
            for (sym, c) in &shared {
                let mut firsts: Vec<f64> = Vec::new();
                for m in members.iter().take(12) {
                    let key = &keys[kc % keys.len()];
                    kc += 1;
                    let url = format!(
                        "https://api.etherscan.io/v2/api?chainid=1&module=account&action=tokentx\
                         &contractaddress={c}&address={m}&page=1&offset=100&sort=asc&apikey={key}"
                    );
                    if let Some(d) = get_json(&client, &url).await {
                        if let Some(arr) = d["result"].as_array() {
                            if let Some(t) = arr
                                .iter()
                                .find(|t| t["to"].as_str().unwrap_or("").to_lowercase() == *m)
                            {
                                firsts.push(num(&t["timeStamp"]));
                            }
                        }
                    }
                    tokio::time::sleep(Duration::from_millis(220)).await;
                }
                if firsts.len() < 2 {
                    continue;
                }
                firsts.sort_by(|a, b| a.partial_cmp(b).unwrap());
                // Найщільніше вікно: скільки членів увійшли протягом SYNC_DAYS одне від одного.
                let win = sync_days * 86400.0;
                let (mut best, mut lo) = (1usize, 0usize);
                for hi in 0..firsts.len() {
                    while firsts[hi] - firsts[lo] > win {
                        lo += 1;
                    }
                    best = best.max(hi - lo + 1);
                }
                sync.push(json!({
                    "symbol": sym,
                    "measured": firsts.len(),
                    "inWindow": best,
                    "spreadDays": (firsts[firsts.len() - 1] - firsts[0]) / 86400.0,
                }));
            }
            // Синхронність має покривати ГРУПУ, а не трійку її членів. Перший прогін
            // назвав EG1 скоординованою на підставі 3 із 132 гаманців — це не доказ,
            // а розмір вибірки. Вимагаємо і абсолютну масу, і частку від групи.
            let need = 5.0f64.max(members.len() as f64 * 0.25);
            let synced = sync.iter().any(|s| {
                num(&s["inWindow"]) >= need
                    && num(&s["inWindow"]) / num(&s["measured"]).max(1.0) >= 0.5
            });
            g["syncEntry"] = json!(sync);
            g["syncNeeded"] = json!(need);
            g["coordinated"] = json!(synced);
        }
    }

    // ── 6. РЕАЛЬНІ ГРОШІ ────────────────────────────────────────────────────
    //
    // «Частка емісії × капіталізація» — паперова оцінка: вона однаково рахує $1М у USDC
    // і $1М у токені, який неможливо продати без обвалу власної ціни. Тому окремо беремо
    // фактичні баланси кожного члена й ділимо на три кошики: стейбли (готівка), ETH
    // (ліквідне ядро) і решта токенів. Розрив між paperUsd і realUsd — і є міра того,
    // наскільки група «багата» лише на папері.
    const STABLES: [&str; 24] = [
        "USDT", "USDC", "DAI", "USDS", "PYUSD", "USDE", "SUSDE", "SUSDS", "RLUSD", "FDUSD",
        "TUSD", "USDG", "USD1", "USDY", "USYC", "BUIDL", "USDP", "GUSD", "LUSD", "CRVUSD",
        "FRAX", "USDD", "EURC", "EURS",
    ];
    println!("[emgroups] рахую реальні баланси членів груп");
    let sem2 = Arc::new(Semaphore::new(conc));
    for g in out.iter_mut() {
        let members: Vec<String> = g["members"]
            .as_array()
            .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();
        let mut futs = Vec::new();
        for m in members.iter().take(250).cloned() {
            let (sem2, client) = (sem2.clone(), client.clone());
            futs.push(tokio::spawn(async move {
                let _p = sem2.acquire().await.ok()?;
                let mut stable = 0.0f64;
                let mut other = 0.0f64;
                if let Some(d) =
                    get_json(&client, &format!("{BS}/api/v2/addresses/{m}/token-balances")).await
                {
                    for it in d.as_array().unwrap_or(&vec![]) {
                        let t = &it["token"];
                        if t["type"].as_str().unwrap_or("") != "ERC-20" {
                            continue;
                        }
                        let rate = num(&t["exchange_rate"]);
                        if rate <= 0.0 {
                            continue;
                        }
                        let dec = num(&t["decimals"]).clamp(0.0, 36.0);
                        let usd = num(&it["value"]) / 10f64.powf(dec) * rate;
                        if !usd.is_finite() || usd <= 0.0 {
                            continue;
                        }
                        let sym = t["symbol"].as_str().unwrap_or("").to_uppercase();
                        if STABLES.contains(&sym.as_str()) {
                            stable += usd;
                        } else {
                            other += usd;
                        }
                    }
                }
                let mut eth = 0.0f64;
                if let Some(d) = get_json(&client, &format!("{BS}/api/v2/addresses/{m}")).await {
                    let rate = num(&d["exchange_rate"]);
                    eth = num(&d["coin_balance"]) / 1e18 * rate;
                }
                Some((stable, eth, other))
            }));
        }
        let (mut st, mut et, mut ot, mut n) = (0.0, 0.0, 0.0, 0usize);
        for f in futs {
            if let Ok(Some((s, e, o))) = f.await {
                st += s;
                et += e;
                ot += o;
                n += 1;
            }
        }
        g["realUsd"] = json!(st + et + ot);
        g["stableUsd"] = json!(st);
        g["ethUsd"] = json!(et);
        g["otherTokenUsd"] = json!(ot);
        g["balancesMeasured"] = json!(n);
    }

    let result = json!({
        "generated": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
        "params": {"topHolders": toph, "minAssets": min_assets, "minWeight": min_w, "syncDays": sync_days},
        "assetsScanned": total,
        "walletsMultiAsset": cands.len(),
        "null": {
            "iters": null_groups.len(),
            "groupsMean": null_groups.iter().sum::<usize>() as f64 / null_groups.len().max(1) as f64,
            "groupsMax": null_groups.iter().max().copied().unwrap_or(0),
            "largestMean": null_max.iter().sum::<usize>() as f64 / null_max.len().max(1) as f64,
            "largestMax": null_max.iter().max().copied().unwrap_or(0),
            "observedGroups": clusters.len(),
            "observedLargest": clusters.first().map(|c| c.len()).unwrap_or(0),
        },
        "groups": out,
    });
    fs::write(
        format!("{dir}/emgroups.json"),
        serde_json::to_string_pretty(&result).unwrap_or_default(),
    )
    .ok();
    fs::write(format!("{dir}/emgroups-report.md"), report(&result)).ok();
    println!("[emgroups] готово → {dir}/emgroups.json + emgroups-report.md");
}

fn report(r: &Value) -> String {
    let empty = vec![];
    let gs = r["groups"].as_array().unwrap_or(&empty);
    let mut s = String::new();
    s.push_str("# Скоординовані групи капіталу серед активів із викупленою емісією\n\n");
    s.push_str(&format!(
        "Активів у вибірці: {} · гаманців із ≥{} спільними активами: {} · груп: {}\n\n",
        r["assetsScanned"],
        r["params"]["minAssets"],
        r["walletsMultiAsset"],
        gs.len()
    ));
    s.push_str(
        "«Скоординована» = група, чиї члени не лише тримають один кошик, а й ЗАХОДИЛИ в \
         нього синхронно. Групи без цієї позначки — статистично щільні кошики, тобто \
         кореляція, яка ще не доведена як змова.\n\n",
    );
    let n = &r["null"];
    s.push_str(&format!(
        "**Нуль-тест** ({} перестановок зі збереженням степенів): випадок дає {:.1} груп \
         у середньому (макс {}), найбільша {:.1} гаманців (макс {}). Спостережено: {} груп, \
         найбільша {} гаманців. Знахідкою вважається лише те, що помітно перевищує цей фон.\n\n",
        n["iters"],
        num(&n["groupsMean"]),
        n["groupsMax"],
        num(&n["largestMean"]),
        n["largestMax"],
        n["observedGroups"],
        n["observedLargest"],
    ));
    for g in gs {
        let sync = if g["coordinated"] == json!(true) { "✅ синхронний вхід" } else { "⚠️ лише спільний кошик" };
        s.push_str(&format!(
            "## {} — {} гаманців · {}\n\n",
            g["id"].as_str().unwrap_or(""),
            g["size"],
            sync
        ));
        s.push_str(&format!(
            "**Реальні гроші: ${:.1}М** — стейбли ${:.1}М · ETH ${:.1}М · інші токени ${:.1}М \
             (виміряно {} гаманців). Паперова оцінка за часткою емісії: ${:.1}М.\n\n",
            num(&g["realUsd"]) / 1e6,
            num(&g["stableUsd"]) / 1e6,
            num(&g["ethUsd"]) / 1e6,
            num(&g["otherTokenUsd"]) / 1e6,
            g["balancesMeasured"],
            num(&g["paperUsd"]) / 1e6,
        ));
        let sa = g["sharedAssets"].as_array().unwrap_or(&empty);
        s.push_str("| актив | сектор | членів тримає | вага |\n|---|---|---:|---:|\n");
        for a in sa.iter().take(8) {
            s.push_str(&format!(
                "| {} | {} | {} | {:.1} |\n",
                a["symbol"].as_str().unwrap_or(""),
                a["sector"].as_str().unwrap_or("—"),
                a["membersHolding"],
                num(&a["weight"])
            ));
        }
        if let Some(sy) = g["syncEntry"].as_array() {
            if !sy.is_empty() {
                s.push_str("\nВхід: ");
                let parts: Vec<String> = sy
                    .iter()
                    .map(|x| {
                        format!(
                            "{} {}/{} за ≤{} дн (розкид {:.0} дн)",
                            x["symbol"].as_str().unwrap_or(""),
                            x["inWindow"],
                            x["measured"],
                            r["params"]["syncDays"],
                            num(&x["spreadDays"])
                        )
                    })
                    .collect();
                s.push_str(&parts.join(" · "));
                s.push('\n');
            }
        }
        let kg = g["knownGroups"].as_array().unwrap_or(&empty);
        let parts: Vec<String> = kg
            .iter()
            .map(|x| format!("{}×{}", x["group"].as_str().unwrap_or(""), x["n"]))
            .collect();
        s.push_str(&format!("\nВідоме розбиття: {}\n\n", parts.join(", ")));
    }
    s
}
