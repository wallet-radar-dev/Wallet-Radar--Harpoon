//! rgbehav — ПОВЕДІНКОВА кластеризація гаманців у ОДНОМУ активі. Координована група = гаманці
//! зі схожим ПАТЕРНОМ (сума-позиція, час входу, тривалість накопичення, час/тривалість розподілу,
//! к-сть угод). Роки-між-входами — норм; групуємо за СХОЖІСТЮ поведінки, не за днем.
//! Лише бекфіл (transfers.csv). Args: ROOT. Env: TOKEN=contract, MIN_CLUSTER (дефолт 3).
//! Вихід: stdout топ-кластери + dataset/backfill/rgbehav-<token>.json.

use serde_json::json;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Write};

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = y - (m <= 2) as i64;
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    era * 146097 + (yoe * 365 + yoe / 4 - yoe / 100 + (153 * mp + 2) / 5 + d - 1) - 719468
}
fn day_of(ts: &str) -> Option<i32> {
    Some(days_from_civil(ts.get(0..4)?.parse().ok()?, ts.get(5..7)?.parse().ok()?, ts.get(8..10)?.parse().ok()?) as i32)
}
fn ym(day: i32) -> (i64, i64) {
    let z = day as i64 + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m)
}
// (б) помʼякшено: КВАРТАЛ замість місяця
fn d2quarter(day: i32) -> String { let (y, m) = ym(day); format!("{:04}-Q{}", y, (m - 1) / 3 + 1) }
fn uf_find(uf: &mut [u32], x: u32) -> u32 { let mut r = x; while uf[r as usize] != r { r = uf[r as usize]; } let mut c = x; while uf[c as usize] != c { let n = uf[c as usize]; uf[c as usize] = r; c = n; } r }
// тир тривалості в днях -> бакет
fn dur_tier(days: i32) -> &'static str {
    match days { d if d < 0 => "-", 0 => "1d", d if d <= 7 => "wk", d if d <= 30 => "mo", d if d <= 90 => "qtr", d if d <= 365 => "yr", _ => "yr+" }
}

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let tokens: std::collections::HashSet<String> = std::env::var("TOKENS").or_else(|_| std::env::var("TOKEN")).expect("env TOKENS")
        .split(',').map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()).collect();
    let min_cluster: usize = std::env::var("MIN_CLUSTER").ok().and_then(|s| s.parse().ok()).unwrap_or(3);
    // мітки символів (для читабельності) з rgbehav не тягнемо; передамо через LABELS="contract=SYM,..."
    let labels: HashMap<String, String> = std::env::var("LABELS").unwrap_or_default().split(',')
        .filter_map(|kv| kv.split_once('=').map(|(k, v)| (k.trim().to_lowercase(), v.trim().to_string()))).collect();

    // per (token, wallet) події — один прохід на всі токени
    let mut ev: HashMap<(String, String), Vec<(i32, f64)>> = HashMap::new();
    let f = File::open(format!("{root}/dataset/backfill/transfers.csv")).expect("transfers.csv");
    for line in BufReader::with_capacity(1 << 20, f).lines().map_while(Result::ok) {
        let mut it = line.splitn(6, ',');
        let w = it.next().unwrap_or("");
        let _c = it.next();
        let dir = it.next().unwrap_or("");
        let ts = it.next().unwrap_or("");
        let tok = it.next().unwrap_or("");
        if w == "wallet" || (dir != "i" && dir != "o") { continue; }
        let tl = tok.to_lowercase();
        if !tokens.contains(&tl) { continue; }
        let mut rit = line.rsplitn(4, ',');
        let _h = rit.next();
        let amt: f64 = rit.next().unwrap_or("0").parse().unwrap_or(0.0);
        if amt <= 0.0 { continue; }
        let Some(day) = day_of(ts) else { continue };
        ev.entry((tl, w.to_lowercase())).or_default().push((day, if dir == "i" { amt } else { -amt }));
    }

    // supply (для концентрації: скільки % емісії тримають кити)
    let mut supply: HashMap<String, (f64, String)> = HashMap::new();
    if let Ok(sv) = serde_json::from_reader::<_, serde_json::Map<String, serde_json::Value>>(BufReader::new(File::open(format!("{root}/dataset/token-supply.json")).unwrap())) {
        for (c, v) in &sv {
            let dec = v["decimals"].as_u64().unwrap_or(18) as i32;
            let raw: f64 = v["supply"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0.0);
            if raw > 0.0 { supply.insert(c.to_lowercase(), (raw / 10f64.powi(dec), v["symbol"].as_str().unwrap_or("").to_string())); }
        }
    }

    struct Feat { tin: f64, tout: f64, fb: i32, lb: i32, fs: i32, ls: i32, ns: u32 }
    let min_coins: usize = std::env::var("MIN_COINS").ok().and_then(|s| s.parse().ok()).unwrap_or(2);
    // інтернер гаманців + крос-монетні пари (ко-кластер) -> множина монет
    let mut wid: HashMap<String, u32> = HashMap::new();
    let mut wname: Vec<String> = Vec::new();
    let mut pair: HashMap<(u32, u32), std::collections::HashSet<String>> = HashMap::new();

    for token in &tokens {
        let sym = labels.get(token).cloned().unwrap_or_else(|| token[..10.min(token.len())].to_string());
        // сигнатура (КВАРТАЛ) -> гаманці(id)
        let mut clusters: HashMap<String, Vec<u32>> = HashMap::new();
        let mut nwal = 0;
        let mut whale_hold = 0.0f64; // сумарний net-холдинг усіх відстежуваних (кити)
        for ((t, w), e) in &ev {
            if t != token { continue; }
            nwal += 1;
            let mut fe = Feat { tin: 0.0, tout: 0.0, fb: i32::MAX, lb: i32::MIN, fs: i32::MAX, ls: i32::MIN, ns: 0 };
            for &(d, s) in e {
                if s > 0.0 { fe.tin += s; fe.fb = fe.fb.min(d); fe.lb = fe.lb.max(d); }
                else { fe.tout += -s; fe.ns += 1; fe.fs = fe.fs.min(d); fe.ls = fe.ls.max(d); }
            }
            whale_hold += (fe.tin - fe.tout).max(0.0);
            if fe.tin <= 0.0 { continue; }
            let sz = fe.tin.log10().floor() as i32;
            let entry = d2quarter(fe.fb);
            let acc = dur_tier(if fe.lb >= fe.fb { fe.lb - fe.fb } else { 0 });
            let dist = if fe.ns > 0 { dur_tier(if fe.ls >= fe.fs { fe.ls - fe.fs } else { 0 }) } else { "hold" };
            let sold = if fe.ns > 0 { "S" } else { "H" };
            let sig = format!("sz10^{sz:<3}|{entry}|acc:{acc:<3}|dist:{dist:<4}|{sold}");
            let id = if let Some(&i) = wid.get(w) { i } else { let i = wname.len() as u32; wname.push(w.clone()); wid.insert(w.clone(), i); i };
            clusters.entry(sig).or_default().push(id);
        }
        let mut cl: Vec<(&String, &Vec<u32>)> = clusters.iter().filter(|(_, v)| v.len() >= min_cluster).collect();
        cl.sort_by(|a, b| b.1.len().cmp(&a.1.len()));
        let conc = if let Some((hs, _)) = supply.get(token) { format!("{:.1}%", (whale_hold / hs).min(1.0) * 100.0) } else { "supply?".into() };
        println!("\n========= {} : {} гаманців → {} груп | 🐋 кити тримають {} емісії =========", sym, nwal, cl.len(), conc);
        for (sig, ws) in cl.iter().take(8) { println!("  {:>5}  {}", ws.len(), sig); }
        // крос-монетні пари з когорт (≥2)
        for ws in clusters.values() {
            if ws.len() < 2 || ws.len() > 200 { continue; }
            for i in 0..ws.len() { for j in i + 1..ws.len() {
                let (a, b) = (ws[i].min(ws[j]), ws[i].max(ws[j]));
                pair.entry((a, b)).or_default().insert(sym.clone());
            }}
        }
    }

    // (а) КРОС-МОНЕТНЕ ЗЛИТТЯ: пара ко-кластериться у ≥MIN_COINS монет → лінк (union-find)
    let n = wname.len() as u32;
    let mut uf: Vec<u32> = (0..n).collect();
    let mut links = 0u64;
    let mut pair_coins: HashMap<(u32, u32), Vec<String>> = HashMap::new();
    for ((a, b), coins) in &pair {
        if coins.len() >= min_coins {
            let (ra, rb) = (uf_find(&mut uf, *a), uf_find(&mut uf, *b));
            if ra != rb { uf[ra as usize] = rb; }
            links += 1;
            pair_coins.insert((*a, *b), coins.iter().cloned().collect());
        }
    }
    let mut comp: HashMap<u32, Vec<u32>> = HashMap::new();
    for i in 0..n { let r = uf_find(&mut uf, i); comp.entry(r).or_default().push(i); }
    let mut groups: Vec<Vec<u32>> = comp.into_values().filter(|v| v.len() >= min_cluster).collect();
    groups.sort_by(|a, b| b.len().cmp(&a.len()));

    println!("\n===== (а) КРОС-МОНЕТНІ СЕРІЙНІ ГРУПИ (ті самі гаманці, схожий патерн у ≥{min_coins} монет) =====");
    println!("груп: {}  (лінків-пар: {})", groups.len(), links);
    let mut out = Vec::new();
    for (i, g) in groups.iter().enumerate() {
        // монети, які групу зв'язують
        let mut coins: std::collections::HashSet<String> = std::collections::HashSet::new();
        for a in 0..g.len() { for b in a + 1..g.len() { let k = (g[a].min(g[b]), g[a].max(g[b])); if let Some(cs) = pair_coins.get(&k) { for c in cs { coins.insert(c.clone()); } } } }
        let mut cv: Vec<String> = coins.into_iter().collect(); cv.sort();
        if i < 20 {
            println!("  Група {}: {} гаманців | монети: {}", i + 1, g.len(), cv.join(","));
            for &w in g.iter().take(4) { println!("       {}", wname[w as usize]); }
        }
        out.push(json!({"id": i + 1, "n_wallets": g.len(), "coins": cv, "wallets": g.iter().take(50).map(|&w| &wname[w as usize]).collect::<Vec<_>>()}));
    }
    File::create(format!("{root}/dataset/backfill/rgbehav-crosscoin.json"))
        .and_then(|mut f| f.write_all(serde_json::to_string_pretty(&json!({"min_coins": min_coins, "n_groups": groups.len(), "groups": out})).unwrap().as_bytes())).ok();
}
