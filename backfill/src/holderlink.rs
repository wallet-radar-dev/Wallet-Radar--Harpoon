//! holderlink — чим саме пов'язані гаманці, що тримають концентровані позиції мережі.
//!
//! `netassets` показав, ЩО тримає мережа (CRV 40% емісії, FRAX 45%, POL 30%, PENDLE 24%),
//! але зберігав лише лічильники. Тут беремо самі списки холдерів цих активів і перевіряємо
//! чотири можливі природи зв'язку, від найслабшої до найсильнішої:
//!
//!   1) СПІЛЬНИЙ КОШИК — скільки з них тримають кілька концентрованих активів одночасно;
//!      найслабший доказ: люди з однаковими смаками не змовники;
//!   2) ЧЛЕНСТВО — як вони розкладені по HUB / NET / NET-WEAK і на яких хопах від ядра;
//!   3) ПРЯМІ ПЕРЕКАЗИ між собою — ребра з eth-edges2 із сумою ≥DUST_ETH (дріб'язок 1e-9
//!      це адресне отруєння, а не стосунки);
//!   4) СПІЛЬНЕ ДЖЕРЕЛО — рідкісний контрагент, з яким має справу 2..CP_MAX із них.
//!      Спільна Binance не пов'язує нікого, тому надто популярні контрагенти відкидаються.
//!
//! Порівняння з фоном обов'язкове: беремо стільки ж випадкових гаманців із того самого
//! розбиття і рахуємо ті самі величини. Без цього «у них 300 спільних контрагентів» —
//! число без сенсу.
//!
//! Args: ROOT EDGES_JSONL. Env: SYMS(CRV,POL,PENDLE,ENS,NEXO,FRAX,USDG,RSETH) TOPH(300)
//!       DUST_ETH(0.001) CP_MAX(50) NULL_ITERS(20)
//! Вихід: dataset/backfill/holderlink.json + holderlink-report.md

use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader};
use std::time::Duration;

const BS: &str = "https://eth.blockscout.com";

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

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let mut args = std::env::args().skip(1);
    let root = args.next().unwrap_or_else(|| "/opt/wallet-tracker".into());
    let edges_path = args
        .next()
        .unwrap_or_else(|| format!("{root}/dataset/backfill/eth-edges2.jsonl"));
    let dir = format!("{root}/dataset/backfill");
    let syms: Vec<String> = std::env::var("SYMS")
        .unwrap_or_else(|_| "CRV,POL,PENDLE,ENS,NEXO,FRAX,USDG,RSETH".into())
        .split(',')
        .map(|s| s.trim().to_uppercase())
        .collect();
    let toph: usize = envn("TOPH", 300);
    let dust: f64 = envn("DUST_ETH", 0.001);
    let cp_max: usize = envn("CP_MAX", 50);
    let null_iters: usize = envn("NULL_ITERS", 20);

    // членство: хто взагалі в мережі
    let known: HashMap<String, String> = serde_json::from_str::<Value>(
        &fs::read_to_string(format!("{dir}/group-membership.json")).expect("group-membership"),
    )
    .ok()
    .and_then(|v| v["wallets"].as_object().cloned())
    .map(|o| {
        o.into_iter()
            .filter_map(|(k, v)| v["group"].as_str().map(|g| (k.to_lowercase(), g.to_string())))
            .collect()
    })
    .unwrap_or_default();
    let in_net = |w: &str| matches!(known.get(w).map(|s| s.as_str()), Some("HUB" | "NET" | "NET-WEAK"));

    // контракти концентрованих активів беремо з netassets
    let na: Value =
        serde_json::from_str(&fs::read_to_string(format!("{dir}/netassets-all59k.json"))
            .or_else(|_| fs::read_to_string(format!("{dir}/netassets.json")))
            .expect("netassets"))
        .expect("json");
    let mut targets: Vec<(String, String)> = Vec::new(); // (symbol, contract)
    for a in na["assets"].as_array().unwrap_or(&vec![]) {
        let s = a["symbol"].as_str().unwrap_or("").to_uppercase();
        if syms.contains(&s) && !targets.iter().any(|(x, _)| *x == s) {
            targets.push((s, a["contract"].as_str().unwrap_or("").to_string()));
        }
    }
    println!("[holderlink] активів знайдено: {} з {}", targets.len(), syms.len());

    let client = reqwest::Client::builder().user_agent("wallet-radar-holderlink/1.0").build().unwrap();

    // ── холдери кожного активу, лише члени мережі ───────────────────────────
    let mut per_asset: Vec<(String, Vec<String>)> = Vec::new();
    for (sym, contract) in &targets {
        let mut hs: Vec<String> = Vec::new();
        let mut params = String::new();
        for _ in 0..(toph.div_ceil(50) + 1) {
            if hs.len() >= toph {
                break;
            }
            let Some(d) = get_json(&client, &format!("{BS}/api/v2/tokens/{contract}/holders{params}")).await
            else {
                break;
            };
            let Some(items) = d["items"].as_array().cloned() else { break };
            if items.is_empty() {
                break;
            }
            for it in &items {
                let w = it["address"]["hash"].as_str().unwrap_or("").to_lowercase();
                if is_burn(&w) || it["address"]["is_contract"].as_bool().unwrap_or(false) {
                    continue;
                }
                if in_net(&w) {
                    hs.push(w);
                }
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
        println!("[holderlink] {sym}: членів мережі у топі — {}", hs.len());
        per_asset.push((sym.clone(), hs));
    }

    // ── 1. спільний кошик ───────────────────────────────────────────────────
    let mut cnt: HashMap<String, Vec<String>> = HashMap::new();
    for (sym, hs) in &per_asset {
        for w in hs {
            cnt.entry(w.clone()).or_default().push(sym.clone());
        }
    }
    let pool: Vec<String> = cnt.keys().cloned().collect();
    let multi: Vec<(&String, &Vec<String>)> = cnt.iter().filter(|(_, v)| v.len() >= 2).collect();
    println!("[holderlink] унікальних гаманців: {} · тримають ≥2 активів: {}", pool.len(), multi.len());

    // ── 2. членство ─────────────────────────────────────────────────────────
    let mut grp: HashMap<&str, usize> = HashMap::new();
    for w in &pool {
        *grp.entry(known.get(w).map(|s| s.as_str()).unwrap_or("—")).or_insert(0) += 1;
    }

    // ── 3-4. ребра й спільні джерела ────────────────────────────────────────
    // Читаємо краул один раз, рахуємо і для цільової групи, і для контрольних вибірок.
    let idx: HashMap<&str, usize> = pool.iter().enumerate().map(|(i, w)| (w.as_str(), i)).collect();
    let all_net: Vec<String> = known
        .iter()
        .filter(|(_, g)| matches!(g.as_str(), "HUB" | "NET" | "NET-WEAK"))
        .map(|(w, _)| w.clone())
        .collect();
    let all_idx: HashMap<&str, usize> = all_net.iter().enumerate().map(|(i, w)| (w.as_str(), i)).collect();

    let mut adj: HashMap<usize, HashSet<usize>> = HashMap::new(); // серед усієї мережі
    let mut cps: HashMap<usize, Vec<String>> = HashMap::new();
    if let Ok(f) = fs::File::open(&edges_path) {
        for line in BufReader::new(f).lines().map_while(Result::ok) {
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            let Some(w) = v["w"].as_str() else { continue };
            let w = w.to_lowercase();
            let Some(&wi) = all_idx.get(w.as_str()) else { continue };
            for cp in v["cps"].as_array().unwrap_or(&vec![]) {
                let Some(a) = cp["a"].as_str() else { continue };
                if num(&cp["i"]) + num(&cp["o"]) < dust {
                    continue;
                }
                let a = a.to_lowercase();
                match all_idx.get(a.as_str()) {
                    Some(&ai) if ai != wi => {
                        adj.entry(wi).or_default().insert(ai);
                        adj.entry(ai).or_default().insert(wi);
                    }
                    _ => cps.entry(wi).or_default().push(a),
                }
            }
        }
    }

    // метрика для довільної вибірки: внутрішніх ребер + пар зі спільним рідкісним джерелом
    let measure = |sample: &[String]| -> (usize, usize) {
        let s: HashSet<usize> = sample.iter().filter_map(|w| all_idx.get(w.as_str()).copied()).collect();
        let mut edges = 0usize;
        for &i in &s {
            if let Some(nb) = adj.get(&i) {
                edges += nb.iter().filter(|j| **j > i && s.contains(*j)).count();
            }
        }
        let mut byc: HashMap<&str, HashSet<usize>> = HashMap::new();
        for &i in &s {
            for c in cps.get(&i).map(|v| v.as_slice()).unwrap_or(&[]) {
                byc.entry(c.as_str()).or_default().insert(i);
            }
        }
        let shared = byc.values().filter(|v| v.len() >= 2 && v.len() <= cp_max).count();
        (edges, shared)
    };

    let (edges, shared) = measure(&pool);
    println!("[holderlink] ЦІЛЬОВІ: внутрішніх ребер {edges} · спільних рідкісних джерел {shared}");

    // контроль: стільки ж випадкових гаманців тієї самої мережі
    let mut seed = 0x2545F4914F6CDD1Du64;
    let mut rnd = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let (mut ce, mut cs) = (0.0f64, 0.0f64);
    for _ in 0..null_iters {
        let mut pick: HashSet<usize> = HashSet::new();
        while pick.len() < pool.len().min(all_net.len()) {
            pick.insert((rnd() as usize) % all_net.len());
        }
        let sample: Vec<String> = pick.iter().map(|&i| all_net[i].clone()).collect();
        let (e, s) = measure(&sample);
        ce += e as f64;
        cs += s as f64;
    }
    ce /= null_iters.max(1) as f64;
    cs /= null_iters.max(1) as f64;
    println!("[holderlink] КОНТРОЛЬ ({null_iters} вибірок): ребер {ce:.0} · спільних джерел {cs:.0}");

    let mut gv: Vec<(&str, usize)> = grp.into_iter().collect();
    gv.sort_by_key(|x| std::cmp::Reverse(x.1));
    let mut mv: Vec<(&String, usize)> = multi.iter().map(|(w, v)| (*w, v.len())).collect();
    mv.sort_by_key(|x| std::cmp::Reverse(x.1));

    let res = json!({
        "assets": per_asset.iter().map(|(s,h)| json!({"symbol": s, "netHolders": h.len()})).collect::<Vec<Value>>(),
        "walletsUnique": pool.len(),
        "multiAsset": multi.len(),
        "membership": gv.iter().map(|(g,n)| json!({"group": g, "n": n})).collect::<Vec<Value>>(),
        "target": {"edges": edges, "sharedSources": shared},
        "control": {"edges": ce, "sharedSources": cs, "iters": null_iters},
        "topMulti": mv.iter().take(20).map(|(w,n)| json!({"wallet": w, "assets": n, "list": cnt[*w]})).collect::<Vec<Value>>(),
    });
    fs::write(format!("{dir}/holderlink.json"), serde_json::to_string_pretty(&res).unwrap()).ok();

    let mut md = String::new();
    md.push_str("# Чим пов'язані гаманці з концентрованими позиціями\n\n");
    md.push_str(&format!(
        "Активи: {} · унікальних гаманців мережі в їхніх топах: **{}** · тримають ≥2 активів: **{}**\n\n",
        syms.join(", "),
        pool.len(),
        multi.len()
    ));
    md.push_str("| Природа зв'язку | цільові | контроль (стільки ж випадкових із мережі) | ×фон |\n|---|---:|---:|---:|\n");
    md.push_str(&format!(
        "| прямі перекази між собою | {edges} | {ce:.0} | ×{:.1} |\n",
        if ce > 0.0 { edges as f64 / ce } else { 0.0 }
    ));
    md.push_str(&format!(
        "| спільні рідкісні джерела | {shared} | {cs:.0} | ×{:.1} |\n\n",
        if cs > 0.0 { shared as f64 / cs } else { 0.0 }
    ));
    md.push_str("Членство: ");
    md.push_str(&gv.iter().map(|(g, n)| format!("{g}×{n}")).collect::<Vec<String>>().join(", "));
    md.push_str("\n\n## Гаманці з найширшим перетином кошика\n\n");
    for (w, n) in mv.iter().take(15) {
        md.push_str(&format!("- `{}` — {} активів: {}\n", w, n, cnt[*w].join(", ")));
    }
    fs::write(format!("{dir}/holderlink-report.md"), md).ok();
    println!("[holderlink] готово → {dir}/holderlink-report.md");
}
