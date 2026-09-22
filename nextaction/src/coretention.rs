//! coretention: групи КООРДИНОВАНОГО РЕТЕНШЕНУ — незв'язані кити, що СПІЛЬНО ТРИМАЮТЬ
//! (net>0, значуща позиція >=MIN_HOLD_USD) ту саму РІДКІСНУ (IDF) множину активів.
//! На відміну від entry-timing: тут ребро лише якщо ОБИДВА досі ТРИМАЮТЬ токен -> група
//! за визначенням контролює частку емісії. union-find -> групи.
//! Focused-whale (kind==whale, n_tokens<=EX_MAXTOK; біржі/агрегатори виключені).
//! Вихід: wallet-groups-emission.json (для emission-біна) + прямий звіт емісії груп.
//! Env: MIN_HOLD_USD(10000) MIN_SHARED(2) EDGE_MIN(0.6) POP(30) EX_MAXTOK(90).

use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader};

fn envu(k: &str, d: usize) -> usize { std::env::var(k).ok().and_then(|s| s.parse().ok()).unwrap_or(d) }
fn envf(k: &str, d: f64) -> f64 { std::env::var(k).ok().and_then(|s| s.parse().ok()).unwrap_or(d) }

struct Uf { p: Vec<u32> }
impl Uf {
    fn new(n: usize) -> Self { Uf { p: (0..n as u32).collect() } }
    fn find(&mut self, x: u32) -> u32 { let mut r = x; while self.p[r as usize] != r { r = self.p[r as usize]; } let mut c = x; while self.p[c as usize] != r { let n = self.p[c as usize]; self.p[c as usize] = r; c = n; } r }
    fn union(&mut self, a: u32, b: u32) { let (ra, rb) = (self.find(a), self.find(b)); if ra != rb { self.p[ra as usize] = rb; } }
}

fn latest_price(path: &std::path::Path) -> Option<f64> {
    let v: Value = serde_json::from_reader(BufReader::new(File::open(path).ok()?)).ok()?;
    let arr = v.get("prices")?.as_array()?;
    let mut bt = i64::MIN; let mut bp = None;
    for e in arr { let t = e["timestamp"].as_i64().unwrap_or(i64::MIN); if t >= bt { if let Some(p) = e["price"].as_f64() { bt = t; bp = Some(p); } } }
    bp
}

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let min_hold_usd = envf("MIN_HOLD_USD", 10_000.0);
    let min_shared = envu("MIN_SHARED", 2) as u32;
    let edge_min = envf("EDGE_MIN", 0.6);
    let pop = envu("POP", 30);
    let ex_maxtok = envu("EX_MAXTOK", 90) as u64;

    // ціни eth: contract -> latest; supply: contract -> human_supply
    let mut price: HashMap<String, f64> = HashMap::new();
    if let Ok(rd) = fs::read_dir(format!("{root}/dataset/nextaction/prices")) {
        for e in rd.flatten() {
            let f = e.file_name().to_string_lossy().to_string();
            let Some(stem) = f.strip_suffix(".json") else { continue };
            let Some((c, a)) = stem.split_once('_') else { continue };
            if c != "eth" { continue; }
            if let Some(p) = latest_price(&e.path()) { price.insert(a.to_lowercase(), p); }
        }
    }
    let mut supply: HashMap<String, (f64, String)> = HashMap::new();
    if let Ok(v) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(
        File::open(format!("{root}/dataset/token-supply.json")).unwrap())) {
        for (c, x) in v {
            let dec = x["decimals"].as_u64().unwrap_or(18) as i32;
            let raw: f64 = x["supply"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0.0);
            if raw > 0.0 { supply.insert(c.to_lowercase(), (raw / 10f64.powi(dec), x["symbol"].as_str().unwrap_or("").to_string())); }
        }
    }
    eprintln!("[coret] eth-цін: {}; supply: {}", price.len(), supply.len());

    // focused whales
    let mut ntok: HashMap<String, u64> = HashMap::new();
    if let Ok(f) = File::open(format!("{root}/dataset/backfill/wallet-enrichment.jsonl")) {
        for l in BufReader::new(f).lines().map_while(Result::ok) {
            if let Ok(v) = serde_json::from_str::<Value>(&l) { if let Some(w) = v["w"].as_str() { ntok.insert(w.to_lowercase(), v["n_tokens"].as_u64().unwrap_or(0)); } }
        }
    }
    let mut whales: HashSet<String> = HashSet::new();
    if let Ok(v) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(
        File::open(format!("{root}/dataset/backfill/wallet-kind.json")).unwrap())) {
        for (a, k) in v { if k.as_str() == Some("whale") { let al = a.to_lowercase(); if ntok.get(&al).copied().unwrap_or(0) <= ex_maxtok { whales.insert(al); } } }
    }
    eprintln!("[coret] focused whale-EOA: {}", whales.len());

    // net holding per (whale, token eth priced)
    let mut net: HashMap<(String, String), f64> = HashMap::new();
    if let Ok(f) = File::open(format!("{root}/dataset/backfill/transfers.csv")) {
        for line in BufReader::new(f).lines().map_while(Result::ok) {
            let mut fr = line.split(',');
            let w = fr.next().unwrap_or("");
            let chain = fr.next().unwrap_or("");
            if chain != "eth" { continue; }
            let dir = fr.next().unwrap_or("");
            let _ts = fr.next();
            let tok = fr.next().unwrap_or("");
            if dir != "i" && dir != "o" { continue; }
            let wl = w.to_lowercase();
            if !whales.contains(&wl) { continue; }
            let tl = tok.to_lowercase();
            if !price.contains_key(&tl) { continue; }
            let mut rr = line.rsplitn(3, ',');
            let _h = rr.next();
            let amt: f64 = rr.next().unwrap_or("0").parse().unwrap_or(0.0);
            if amt == 0.0 { continue; }
            *net.entry((wl, tl)).or_insert(0.0) += if dir == "i" { amt } else { -amt };
        }
    }

    // retention: token -> set of whale-id, що ТРИМАЮТЬ значущу позицію
    let mut wid: HashMap<String, u32> = HashMap::new();
    let mut waddr: Vec<String> = Vec::new();
    let mut sym: HashMap<String, String> = HashMap::new();
    let mut tok_holders: HashMap<String, Vec<(u32, f64)>> = HashMap::new(); // token -> (wid, net_amt)
    for ((w, tk), &n) in &net {
        if n <= 0.0 { continue; }
        let usd = n * price.get(tk).copied().unwrap_or(0.0);
        if usd < min_hold_usd { continue; } // значуща позиція, не пил
        let id = *wid.entry(w.clone()).or_insert_with(|| { let i = waddr.len() as u32; waddr.push(w.clone()); i });
        tok_holders.entry(tk.clone()).or_default().push((id, n));
        if let Some((_, s)) = supply.get(tk) { sym.entry(tk.clone()).or_insert(s.clone()); }
    }
    let nw = waddr.len();
    eprintln!("[coret] китів зі значущим ретеншеном: {nw}; токенів утримання: {}", tok_holders.len());

    // co-retention пари (IDF), токени з 2..=POP ретейнерами
    let mut pair: HashMap<(u32, u32), (f64, u32)> = HashMap::new();
    let mut pair_tok: HashMap<(u32, u32), Vec<String>> = HashMap::new();
    for (tk, hs) in &tok_holders {
        let ids: Vec<u32> = { let mut v: Vec<u32> = hs.iter().map(|(i, _)| *i).collect(); v.sort(); v.dedup(); v };
        if ids.len() < 2 || ids.len() > pop { continue; }
        let idf = (nw as f64 / ids.len() as f64).ln().max(0.0);
        for i in 0..ids.len() { for j in i + 1..ids.len() {
            let k = (ids[i], ids[j]);
            let e = pair.entry(k).or_insert((0.0, 0)); e.0 += idf; e.1 += 1;
            pair_tok.entry(k).or_default().push(tk.clone());
        } }
    }

    let mut uf = Uf::new(nw);
    for (&(a, b), &(sc, cnt)) in &pair { if cnt >= min_shared && sc >= edge_min { uf.union(a, b); } }

    // групи + пряма частка емісії
    let mut comp: HashMap<u32, Vec<u32>> = HashMap::new();
    for i in 0..nw as u32 { comp.entry(uf.find(i)).or_default().push(i); }
    let hold_of: HashMap<(u32, String), f64> = {
        let mut m = HashMap::new();
        for (tk, hs) in &tok_holders { for (id, n) in hs { m.insert((*id, tk.clone()), *n); } }
        m
    };
    let mut groups: Vec<Value> = Vec::new();
    for (_, mem) in &comp {
        if mem.len() < 2 { continue; }
        let mset: HashSet<u32> = mem.iter().copied().collect();
        // co-retained токени групи -> combined emission share
        let mut tshare: Vec<(String, f64, u32)> = Vec::new();
        for (tk, hs) in &tok_holders {
            let members: Vec<&(u32, f64)> = hs.iter().filter(|(i, _)| mset.contains(i)).collect();
            if members.len() < 2 { continue; } // >=2 члени ТРИМАЮТЬ -> спільний контроль
            let combined: f64 = members.iter().map(|(_, n)| *n).sum();
            if let Some((hsupply, s)) = supply.get(tk) {
                // Фаза 0.2: частка емісії — зі СЛОВНИКА (hsupply>0 гарант.) → байт-паритет.
                let share = wr_dict::emission_share(combined, *hsupply).min(1.0);
                tshare.push((s.clone(), share, members.len() as u32));
            } else {
                tshare.push((sym.get(tk).cloned().unwrap_or_else(|| tk[..8.min(tk.len())].to_string()), -1.0, members.len() as u32));
            }
        }
        if tshare.is_empty() { continue; }
        tshare.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        let wallets: Vec<String> = mem.iter().map(|&i| waddr[i as usize].clone()).collect();
        let toks: Vec<Value> = tshare.iter().take(15).map(|(s, sh, n)| json!({"symbol": s, "emission_share": (sh*1e6).round()/1e6, "n_holding": n})).collect();
        let top_tags: Vec<String> = { // для формату emission-біна
            let mut v: HashMap<String, u32> = HashMap::new();
            for x in 0..mem.len() { for y in x+1..mem.len() { let k = if mem[x]<mem[y] {(mem[x],mem[y])} else {(mem[y],mem[x])}; if let Some(ts)=pair_tok.get(&k){ for t in ts { *v.entry(sym.get(t).cloned().unwrap_or_default()).or_insert(0)+=1; } } } }
            let mut vv: Vec<(String,u32)> = v.into_iter().collect(); vv.sort_by(|a,b| b.1.cmp(&a.1));
            vv.into_iter().take(8).map(|(s,_)| s).collect()
        };
        groups.push(json!({"size": wallets.len(), "wallets": wallets, "topSharedTokens": top_tags, "coRetained": toks}));
    }
    groups.sort_by(|a, b| {
        let ma = a["coRetained"][0]["emission_share"].as_f64().unwrap_or(0.0);
        let mb = b["coRetained"][0]["emission_share"].as_f64().unwrap_or(0.0);
        mb.partial_cmp(&ma).unwrap()
    });

    let path = format!("{root}/dataset/backfill/wallet-groups-emission.json");
    fs::write(&path, serde_json::to_string_pretty(&json!({
        "method": "co-retention (both still hold significant position of same rare token, IDF)",
        "params": {"MIN_HOLD_USD": min_hold_usd, "MIN_SHARED": min_shared, "EDGE_MIN": edge_min, "POP": pop, "EX_MAXTOK": ex_maxtok},
        "totalGroups": groups.len(), "groups": groups
    })).unwrap()).expect("write");

    eprintln!("\n=== КО-РЕТЕНШЕН ГРУПИ (спільно тримають рідкісний кошик) ===");
    for g in groups.iter().take(20) {
        let ct: Vec<String> = g["coRetained"].as_array().unwrap().iter().take(6)
            .map(|t| { let sh = t["emission_share"].as_f64().unwrap(); format!("{}({}, {}h)", t["symbol"].as_str().unwrap(), if sh<0.0 {"n/a".into()} else {format!("{:.2}%", sh*100.0)}, t["n_holding"].as_u64().unwrap()) }).collect();
        eprintln!("  size={:<3} co-retained: {}", g["size"].as_u64().unwrap(), ct.join(" "));
    }
    eprintln!("\nгруп: {}; вихід: {path}", groups.len());
}
