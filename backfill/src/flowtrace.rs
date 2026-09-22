//! flowtrace — трасування КОНКРЕТНОГО потоку: куди він розпадається і де сходиться знову.
//!
//! Навіщо: класифікація за формою («воронка», «маска») механізму розмиття не довела — під
//! «воронки» потрапили жертви спам-розсилок (UniLife, ChatGpt, ha138com), а маски мають
//! сумарний приплив $24 751, тобто нічого не розмивають. Тому інший підхід: узяти реальний
//! вихід із відомого центру й простежити його вперед по хопах.
//!
//! Розмиття виглядає так: сума йде на N адрес, кожна ділить далі, а потім гілки СХОДЯТЬСЯ
//! на спільних адресах. Саме сходження — доказ, що поділ був навмисним, а не природним
//! розсіюванням. Тому головна метрика тут — вузли, у які прийшло від ≥2 РІЗНИХ гілок.
//!
//! Трасуємо ETH (у нього видно суму), лише перекази ≥ MIN_ETH, у вікні DAYS після старту.
//!
//! Вихід: dataset/backfill/flowtrace.json. Args: ROOT START_ADDR
//! Env: MIN_ETH(1.0) HOPS(4) DAYS(90) CHAIN(1)
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, Write};

fn envn<T: std::str::FromStr>(k: &str, d: T) -> T {
    std::env::var(k).ok().and_then(|s| s.parse().ok()).unwrap_or(d)
}

#[derive(Clone)]
struct Mv { from: String, to: String, eth: f64, ts: u64 }

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let start = std::env::args().nth(2).expect("вкажи стартову адресу").to_lowercase();
    let bf = format!("{root}/dataset/backfill");
    let chain: u32 = envn("CHAIN", 1);
    let min_eth: f64 = envn("MIN_ETH", 1.0);
    let hops: usize = envn("HOPS", 4);
    let days: u64 = envn("DAYS", 90);

    let mut labels: HashMap<String, String> = HashMap::new();
    if let Ok(f) = File::open(format!("{bf}/labels-cache.json")) {
        if let Ok(m) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(f)) {
            for (a, v) in m {
                let mut s = v["name"].as_str().unwrap_or("").to_string();
                if let Some(arr) = v["labels"].as_array() {
                    for x in arr { if let Some(t) = x.as_str() { s.push(' '); s.push_str(t) } }
                }
                if !s.trim().is_empty() { labels.insert(a.to_lowercase(), s); }
            }
        }
    }
    let members: HashSet<String> = BufReader::new(File::open(format!("{bf}/net-members.txt")).unwrap())
        .lines().map_while(Result::ok).map(|l| l.trim().to_lowercase()).collect();

    // --- один прохід: усі ETH-перекази ≥min_eth, де відправник нам може знадобитись ---
    // Тримаємо повний список рухів: 63.9 млн рядків, але після фільтра за сумою лишається
    // на порядки менше, і це дозволяє трасувати без повторних проходів по файлу.
    let mut moves: HashMap<String, Vec<Mv>> = HashMap::new();
    let rdr = BufReader::with_capacity(1 << 22,
        File::open(format!("{bf}/txfull-{chain}.jsonl")).expect("txfull"));
    let mut seen: HashSet<u64> = HashSet::new();
    let mut cur_w = String::new();
    let (mut n, mut kept) = (0u64, 0u64);

    for line in rdr.lines().map_while(Result::ok) {
        n += 1;
        if n % 20_000_000 == 0 { eprintln!("    ...{n} рядків, рухів {kept}") }
        if !line.contains("\"k\":\"ext\"") && !line.contains("\"k\":\"int\"") { continue }
        let Ok(t) = serde_json::from_str::<Value>(&line) else { continue };
        let w = t["w"].as_str().unwrap_or("");
        if w != cur_w { cur_w = w.to_string(); seen.clear() }
        let v: f64 = t["v"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0.0) / 1e18;
        if v < min_eth { continue }
        let f = t["f"].as_str().unwrap_or("").to_lowercase();
        let to = t["t"].as_str().unwrap_or("").to_lowercase();
        if f.is_empty() || to.is_empty() || f == to { continue }
        let ts = t["ts"].as_u64().unwrap_or(0);
        let mut h: u64 = 1469598103934665603;
        for s in [t["h"].as_str().unwrap_or(""), f.as_str(), to.as_str(), t["v"].as_str().unwrap_or("")] {
            for b in s.as_bytes() { h ^= *b as u64; h = h.wrapping_mul(1099511628211) }
        }
        if !seen.insert(h) { continue }
        kept += 1;
        moves.entry(f.clone()).or_default().push(Mv { from: f, to, eth: v, ts });
    }
    eprintln!("[flowtrace] рядків {n} · рухів ≥{min_eth} ETH: {kept} · відправників {}", moves.len());

    // --- BFS уперед від старту ---
    let t0 = moves.get(&start).map(|v| v.iter().map(|m| m.ts).min().unwrap_or(0)).unwrap_or(0);
    let t_end = if t0 > 0 { t0 + days * 86400 } else { u64::MAX };
    let mut level: Vec<(String, f64)> = vec![(start.clone(), 0.0)];
    let mut visited: HashSet<String> = HashSet::from([start.clone()]);
    // хто прийшов у вузол і з якої гілки — для пошуку сходжень
    let mut arrivals: HashMap<String, HashSet<String>> = HashMap::new();
    let mut report: Vec<Value> = Vec::new();
    let mut to_cex_total = 0.0;

    for hop in 1..=hops {
        let mut next: HashMap<String, f64> = HashMap::new();
        let mut edges = 0u32;
        let mut hop_sum = 0.0;
        for (a, _) in &level {
            let Some(ms) = moves.get(a) else { continue };
            for m in ms {
                if m.ts < t0 || m.ts > t_end { continue }
                edges += 1;
                hop_sum += m.eth;
                *next.entry(m.to.clone()).or_insert(0.0) += m.eth;
                arrivals.entry(m.to.clone()).or_default().insert(a.clone());
                if labels.get(&m.to).map(|l| {
                    let l = l.to_lowercase();
                    ["binance","coinbase","okx","kraken","bybit","kucoin","bitfinex","huobi",
                     "mexc","crypto.com","bithumb","gemini","bitstamp","deposit","otc"]
                        .iter().any(|w| l.contains(w))
                }).unwrap_or(false) { to_cex_total += m.eth }
            }
        }
        let mut fresh: Vec<(String, f64)> = next.into_iter()
            .filter(|(a, _)| visited.insert(a.clone())).collect();
        fresh.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        eprintln!("[flowtrace] хоп {hop}: ребер {edges} · нових адрес {} · обсяг {:.1} ETH",
            fresh.len(), hop_sum);
        report.push(json!({"hop": hop, "edges": edges, "newAddrs": fresh.len(), "eth": hop_sum,
            "top": fresh.iter().take(10).map(|(a, v)| json!({"addr": a, "eth": v,
                "label": labels.get(a), "member": members.contains(a)})).collect::<Vec<_>>()}));
        if fresh.is_empty() { break }
        level = fresh;
    }

    // --- СХОДЖЕННЯ: куди прийшло від кількох різних гілок ---
    let mut conv: Vec<(&String, usize)> = arrivals.iter()
        .filter(|(_, s)| s.len() >= 2).map(|(a, s)| (a, s.len())).collect();
    conv.sort_by(|a, b| b.1.cmp(&a.1));

    eprintln!("\n=== СХОДЖЕННЯ ГІЛОК (доказ навмисного поділу) ===");
    if conv.is_empty() {
        eprintln!("  сходжень немає — потік просто розсіявся");
    }
    for (a, c) in conv.iter().take(20) {
        let l = labels.get(*a).map(|s| s.chars().take(30).collect::<String>()).unwrap_or_default();
        eprintln!("  {} <- від {} різних гілок {} {}", &a[..20.min(a.len())], c,
            if members.contains(*a) { "[ЧЛЕН]" } else { "" }, l);
    }
    eprintln!("\nна біржі/OTC у межах трасування: {:.1} ETH", to_cex_total);

    let _ = File::create(format!("{bf}/flowtrace.json")).unwrap().write_all(
        serde_json::to_string(&json!({"start": start, "minEth": min_eth, "hops": hops,
            "days": days, "toCexEth": to_cex_total, "levels": report,
            "convergence": conv.iter().take(100).map(|(a, c)| json!({"addr": a, "branches": c,
                "label": labels.get(*a), "member": members.contains(*a)})).collect::<Vec<_>>()
        })).unwrap().as_bytes());
    eprintln!("\n[flowtrace] збережено flowtrace.json");
}
