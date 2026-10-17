//! seedcohort — когорта капіталу в активах, які відібрали спостерігачі за китами.
//!
//! Вхід: seedscan.json. Але «обидва сіди торкнулися» саме по собі непридатне як фільтр:
//! 273 з 324 таких активів виявились Polygon-спамом (`US$`, `ERC20`, `Earn nonstop!`,
//! `AETH [ WWW.20E…`, гомогліф `PỌL`). Фішингові ейрдропи розпорошують на тисячі активних
//! адрес, тому перетин двох активних гаманців на них гарантований і нічого не означає.
//! Тому кожен актив проходить перевірку на реальність: ринкова ціна, база власників,
//! осмислений символ.
//!
//! Далі — той самий апарат, що й у решті сесії, з усіма набитими ґулями:
//!   • контракти, спалені адреси й біржова кастодія у холдери не йдуть;
//!   • вага спільного активу = IDF (рідкісний важить більше, спам знецінюється сам);
//!   • ребро вимагає ≥MIN_LINKS спільних активів;
//!   • НУЛЬ-ТЕСТ перестановками зі збереженням степенів — без нього будь-яка таблиця
//!     груп у цьому проєкті вже двічі виявлялась шумом.
//!
//! Args: ROOT. Env: MIN_HOLDERS(300) TOPH(300) MIN_LINKS(2) MIN_SCORE(8) MIN_GROUP(3)
//!       BOTH_ONLY(0) CONC(6) NULL_ITERS(10)
//! Вихід: dataset/backfill/seedcohort.json + seedcohort-report.md

use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;

fn host(chain: &str) -> Option<&'static str> {
    Some(match chain {
        "eth" => "https://eth.blockscout.com",
        "base" => "https://base.blockscout.com",
        "arbitrum" => "https://arbitrum.blockscout.com",
        "optimism" => "https://optimism.blockscout.com",
        "polygon" => "https://polygon.blockscout.com",
        "zksync" => "https://zksync.blockscout.com",
        "linea" => "https://explorer.linea.build",
        "blast" => "https://blast.blockscout.com",
        "scroll" => "https://scroll.blockscout.com",
        "mantle" => "https://mantle.blockscout.com",
        "gnosis" => "https://gnosis.blockscout.com",
        "celo" => "https://celo.blockscout.com",
        _ => return None,
    })
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
const CEX: [&str; 22] = [
    "binance", "coinbase", "kraken", "okx", "bybit", "kucoin", "huobi", "htx", "bitfinex",
    "upbit", "bithumb", "mexc", "bitget", "gate", "crypto-com", "bitstamp", "gemini",
    "poloniex", "bingx", "paribu", "exchange", "hot-wallet",
];
fn custodial(name: &str, tags: &Value) -> bool {
    let mut h = name.to_lowercase();
    if let Some(a) = tags.as_array() {
        for t in a {
            h.push(' ');
            h.push_str(&t["slug"].as_str().unwrap_or("").to_lowercase());
        }
    }
    CEX.iter().any(|s| h.contains(s))
}

/// Символ спам-токена майже завжди видає себе сам: домен у назві, пробіли, дужки,
/// заклики до дії, не-ASCII гомогліфи. Легітимний тікер — короткий і алфавітно-цифровий.
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

async fn get_json(c: &reqwest::Client, url: &str) -> Option<Value> {
    // Ліміт Etherscan приходить як HTTP 200 з {"message":"NOTOK","result":"Max rate limit reached"} —
    // перевірки статусу замало, відказ треба ловити в ТІЛІ, інакше він читається як порожній результат.
    for attempt in 0..4u64 {
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
        tokio::time::sleep(Duration::from_millis(800 * (attempt + 1))).await;
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

fn cluster(bt: &[Vec<usize>], w: &[f64], n: usize, min_links: usize, min_score: f64, min_group: usize) -> Vec<Vec<usize>> {
    let mut pair: HashMap<(usize, usize), (f64, usize)> = HashMap::new();
    for (ti, hs) in bt.iter().enumerate() {
        if w[ti] <= 0.0 || hs.len() > 600 {
            continue;
        }
        for i in 0..hs.len() {
            for j in (i + 1)..hs.len() {
                let k = (hs[i].min(hs[j]), hs[i].max(hs[j]));
                let e = pair.entry(k).or_insert((0.0, 0));
                e.0 += w[ti];
                e.1 += 1;
            }
        }
    }
    let mut uf = Uf::new(n);
    for ((a, b), (s, c)) in &pair {
        if *c >= min_links && *s >= min_score {
            uf.union(*a, *b);
        }
    }
    let mut comp: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..n {
        comp.entry(uf.find(i)).or_default().push(i);
    }
    comp.into_values().filter(|g| g.len() >= min_group).collect()
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let dir = format!("{root}/dataset/backfill");
    let min_holders: f64 = envn("MIN_HOLDERS", 300.0);
    let toph: usize = envn("TOPH", 300);
    let min_links: usize = envn("MIN_LINKS", 2);
    let min_score: f64 = envn("MIN_SCORE", 8.0);
    let min_group: usize = envn("MIN_GROUP", 3);
    let both_only: usize = envn("BOTH_ONLY", 0);
    let conc: usize = envn("CONC", 6);
    let null_iters: usize = envn("NULL_ITERS", 10);

    let src: Value =
        serde_json::from_str(&fs::read_to_string(format!("{dir}/seedscan.json")).expect("seedscan"))
            .expect("json");
    let cands: Vec<Value> = src["assets"]
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .filter(|a| {
            let s = a["symbol"].as_str().unwrap_or("");
            !junk_symbol(s)
                && host(a["chain"].as_str().unwrap_or("")).is_some()
                && (both_only == 0 || num(&a["seeds"]) >= 2.0)
        })
        .cloned()
        .collect();
    println!("[seedcohort] кандидатів після відсіву сміттєвих символів: {}", cands.len());

    let client = reqwest::Client::builder().user_agent("wallet-radar-seedcohort/1.0").build().unwrap();
    let sem = Arc::new(Semaphore::new(conc));
    let ctr = Arc::new(AtomicUsize::new(0));
    let total = cands.len();

    // ── реальність активу + холдери ─────────────────────────────────────────
    let mut futs = Vec::new();
    for a in cands {
        let (sem, client, ctr) = (sem.clone(), client.clone(), ctr.clone());
        futs.push(tokio::spawn(async move {
            let _p = sem.acquire().await.ok()?;
            let chain = a["chain"].as_str()?.to_string();
            let bs = host(&chain)?;
            let c = a["contract"].as_str()?.to_string();
            let info = get_json(&client, &format!("{bs}/api/v2/tokens/{c}")).await?;
            let holders = num(&info["holders_count"]);
            let rate = num(&info["exchange_rate"]);
            // Реальний актив має і ціну, і базу власників. Спам не має ні того, ні того.
            if holders < min_holders || rate <= 0.0 {
                return None;
            }
            let sym = info["symbol"].as_str().unwrap_or(a["symbol"].as_str().unwrap_or("")).to_string();
            let mut hs: Vec<String> = Vec::new();
            let mut params = String::new();
            for _ in 0..(toph.div_ceil(50) + 1) {
                if hs.len() >= toph {
                    break;
                }
                let Some(d) = get_json(&client, &format!("{bs}/api/v2/tokens/{c}/holders{params}")).await
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
                        || custodial(
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
                tokio::time::sleep(Duration::from_millis(150)).await;
            }
            let n = ctr.fetch_add(1, Ordering::Relaxed) + 1;
            if n % 25 == 0 {
                println!("[seedcohort] активів опрацьовано {n}/{total}");
            }
            if hs.len() < 5 {
                return None;
            }
            Some((format!("{sym} ({chain})"), hs))
        }));
    }

    let mut wid: HashMap<String, usize> = HashMap::new();
    let mut wnames: Vec<String> = Vec::new();
    let mut tnames: Vec<String> = Vec::new();
    let mut by_token: Vec<Vec<usize>> = Vec::new();
    for f in futs {
        let Ok(Some((name, hs))) = f.await else { continue };
        let mut ids: Vec<usize> = hs
            .into_iter()
            .map(|w| {
                *wid.entry(w.clone()).or_insert_with(|| {
                    wnames.push(w);
                    wnames.len() - 1
                })
            })
            .collect();
        ids.sort_unstable();
        ids.dedup();
        tnames.push(name);
        by_token.push(ids);
    }
    let n_all = wnames.len();
    println!("[seedcohort] реальних активів: {} · унікальних гаманців: {n_all}", by_token.len());

    // лише мульти-активні; ваги IDF
    let mut deg = vec![0usize; n_all];
    for h in &by_token {
        for &w in h {
            deg[w] += 1;
        }
    }
    let multi = deg.iter().filter(|d| **d >= 2).count();
    println!("[seedcohort] тримають ≥2 активів: {multi}");
    let by_token: Vec<Vec<usize>> = by_token
        .into_iter()
        .map(|h| h.into_iter().filter(|&w| deg[w] >= 2).collect())
        .collect();
    let weight: Vec<f64> = by_token
        .iter()
        .map(|h| if h.len() < 2 { 0.0 } else { (multi as f64 / h.len() as f64).ln().max(0.0) })
        .collect();

    let mut groups = cluster(&by_token, &weight, n_all, min_links, min_score, min_group);
    groups.sort_by_key(|g| std::cmp::Reverse(g.len()));
    println!("[seedcohort] груп: {} · найбільша: {}", groups.len(), groups.first().map(|g| g.len()).unwrap_or(0));

    // ── нуль-тест ───────────────────────────────────────────────────────────
    let mut pairs: Vec<(usize, usize)> = Vec::new();
    for (ti, h) in by_token.iter().enumerate() {
        if weight[ti] > 0.0 {
            for &w in h {
                pairs.push((w, ti));
            }
        }
    }
    let mut seed = 0xA5A5A5A5DEADBEEFu64;
    let mut rnd = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let (mut ng, mut nl) = (0.0f64, 0usize);
    for _ in 0..null_iters {
        let n = pairs.len().max(1);
        for _ in 0..(n * 6) {
            let (i, j) = ((rnd() as usize) % n, (rnd() as usize) % n);
            if pairs[i].1 == pairs[j].1 || pairs[i].0 == pairs[j].0 {
                continue;
            }
            let (a1, a2) = (pairs[i].1, pairs[j].1);
            pairs[i].1 = a2;
            pairs[j].1 = a1;
        }
        let mut bt: Vec<Vec<usize>> = vec![Vec::new(); by_token.len()];
        for (w, t) in &pairs {
            bt[*t].push(*w);
        }
        let g = cluster(&bt, &weight, n_all, min_links, min_score, min_group);
        ng += g.len() as f64;
        nl = nl.max(g.iter().map(|x| x.len()).max().unwrap_or(0));
    }
    ng /= null_iters.max(1) as f64;
    println!(
        "[seedcohort] НУЛЬ: {ng:.1} груп, найбільша до {nl} · спостережено {} груп, найбільша {}",
        groups.len(),
        groups.first().map(|g| g.len()).unwrap_or(0)
    );

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
    let mut wtok: HashMap<usize, Vec<usize>> = HashMap::new();
    for (ti, h) in by_token.iter().enumerate() {
        for &w in h {
            wtok.entry(w).or_default().push(ti);
        }
    }

    let out: Vec<Value> = groups
        .iter()
        .take(25)
        .enumerate()
        .map(|(i, g)| {
            let mut b: HashMap<usize, usize> = HashMap::new();
            let mut kg: HashMap<&str, usize> = HashMap::new();
            for &w in g {
                for t in wtok.get(&w).map(|v| v.as_slice()).unwrap_or(&[]) {
                    *b.entry(*t).or_insert(0) += 1;
                }
                *kg.entry(known.get(&wnames[w]).map(|s| s.as_str()).unwrap_or("—")).or_insert(0) += 1;
            }
            let mut bv: Vec<(usize, usize)> = b.into_iter().filter(|x| x.1 >= 2).collect();
            bv.sort_by_key(|x| std::cmp::Reverse(x.1));
            let mut kv: Vec<(&str, usize)> = kg.into_iter().collect();
            kv.sort_by_key(|x| std::cmp::Reverse(x.1));
            json!({
                "id": format!("SC{}", i + 1), "size": g.len(),
                "members": g.iter().take(25).map(|&w| wnames[w].as_str()).collect::<Vec<&str>>(),
                "basket": bv.iter().take(10).map(|(t,n)| json!({"asset": tnames[*t], "members": n, "idf": weight[*t]})).collect::<Vec<Value>>(),
                "knownGroups": kv.iter().take(4).map(|(g,n)| json!({"group": g, "n": n})).collect::<Vec<Value>>(),
            })
        })
        .collect();

    let res = json!({
        "assetsReal": by_token.len(), "wallets": n_all, "multiAsset": multi,
        "null": {"groupsMean": ng, "largestMax": nl},
        "observed": {"groups": groups.len(), "largest": groups.first().map(|g| g.len()).unwrap_or(0)},
        "groups": out,
    });
    fs::write(format!("{dir}/seedcohort.json"), serde_json::to_string_pretty(&res).unwrap()).ok();

    let mut md = String::new();
    md.push_str("# Когорта капіталу в активах, відібраних спостерігачами\n\n");
    md.push_str(&format!(
        "Реальних активів (ціна + ≥{min_holders:.0} власників + осмислений тікер): **{}** · \
         гаманців {n_all} · тримають ≥2 активів {multi}\n\n\
         **Нуль-тест**: випадковість дає {ng:.1} груп, найбільша до {nl}. Спостережено {} груп, \
         найбільша {}.\n\n",
        by_token.len(),
        groups.len(),
        groups.first().map(|g| g.len()).unwrap_or(0)
    ));
    for g in res["groups"].as_array().unwrap_or(&vec![]) {
        md.push_str(&format!("## {} — {} гаманців\n\n", g["id"].as_str().unwrap_or(""), g["size"]));
        md.push_str("| актив | членів | IDF |\n|---|---:|---:|\n");
        for b in g["basket"].as_array().unwrap_or(&vec![]) {
            md.push_str(&format!(
                "| {} | {} | {:.1} |\n",
                b["asset"].as_str().unwrap_or(""),
                b["members"],
                num(&b["idf"])
            ));
        }
        let kv: Vec<String> = g["knownGroups"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .map(|x| format!("{}×{}", x["group"].as_str().unwrap_or(""), x["n"]))
            .collect();
        md.push_str(&format!("\nВідоме розбиття: {}\n\n", kv.join(", ")));
    }
    fs::write(format!("{dir}/seedcohort-report.md"), md).ok();
    println!("[seedcohort] готово → {dir}/seedcohort-report.md");
}
