//! Збагачення БД: per-wallet поведінковий профіль з transfers.csv (+transfers-btc.csv)
//! та native-ETH (eth-native-net.json). Один прохід по CSV, агрегати на гаманець,
//! вихід JSONL -> dataset/backfill/wallet-enrichment.jsonl.
//! Схема рядка: wallet(0),chain(1),dir(2),ts(3),token(4),symbol(5),...,amount,tx_hash.
//! Мета: зшити поведінку (активність, диверсифікація, DePIN-нахил, крос-чейн) в одну таблицю.

use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};

#[derive(Default)]
struct W {
    n_in: u32,
    n_out: u32,
    depin_in: u32,
    tokens: HashSet<String>,
    chains: HashSet<String>,
    first: String,
    last: String,
    sym: HashMap<String, u32>,
}

fn depin() -> HashSet<&'static str> {
    ["link","grt","ankr","rlc","pha","audio","woo","render","rndr","fil",
     "hnt","iotx","theta","akt","dimo","io","gomining","peaq","nosana"]
        .into_iter().collect()
}

fn scan(path: &str, ws: &mut HashMap<String, W>, dep: &HashSet<&'static str>) -> u64 {
    let f = match File::open(path) {
        Ok(f) => BufReader::new(f),
        Err(_) => return 0,
    };
    let mut rows = 0u64;
    for line in f.lines().map_while(Result::ok) {
        let mut left = line.splitn(6, ',');
        let wallet = match left.next() { Some(w) => w, None => continue };
        if wallet == wr_store::addrs::zero() || wallet == "wallet" {
            continue;
        }
        let chain = left.next().unwrap_or("");
        let dir = left.next().unwrap_or("?");
        let ts = left.next().unwrap_or("");
        let token = left.next().unwrap_or("");
        if dir != "i" && dir != "o" { continue; }
        let symbol = line.split(',').nth(5).unwrap_or("").to_string();
        if token.is_empty() { continue; }
        rows += 1;
        let w = ws.entry(wallet.to_lowercase()).or_default();
        if dir == "i" {
            w.n_in += 1;
            if dep.contains(symbol.to_lowercase().as_str()) { w.depin_in += 1; }
        } else {
            w.n_out += 1;
        }
        w.tokens.insert(format!("{chain}\t{token}"));
        w.chains.insert(chain.to_string());
        *w.sym.entry(symbol).or_insert(0) += 1;
        if !ts.is_empty() {
            if w.first.is_empty() || ts < w.first.as_str() { w.first = ts.to_string(); }
            if ts > w.last.as_str() { w.last = ts.to_string(); }
        }
    }
    rows
}

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let dep = depin();
    let mut ws: HashMap<String, W> = HashMap::new();
    let r1 = scan(&format!("{root}/dataset/backfill/transfers.csv"), &mut ws, &dep);
    let r2 = scan(&format!("{root}/dataset/backfill/transfers-btc.csv"), &mut ws, &dep);
    eprintln!("оброблено рядків: erc20={r1} btc={r2}; гаманців: {}", ws.len());

    // native-ETH net
    let eth: HashMap<String, f64> = File::open(format!("{root}/dataset/backfill/eth-native-net.json"))
        .ok()
        .and_then(|f| serde_json::from_reader::<_, HashMap<String, f64>>(BufReader::new(f)).ok())
        .map(|m| m.into_iter().map(|(k, v)| (k.to_lowercase(), v)).collect())
        .unwrap_or_default();
    eprintln!("native-ETH записів: {}", eth.len());

    let out = format!("{root}/dataset/backfill/wallet-enrichment.jsonl");
    let mut w = BufWriter::new(File::create(&out).expect("create out"));
    let mut depin_wallets = 0u64;
    for (addr, e) in &ws {
        let mut top: Vec<(&String, &u32)> = e.sym.iter().collect();
        top.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        let top3: Vec<_> = top.iter().take(3).map(|(s, c)| json!([s, c])).collect();
        if e.depin_in > 0 { depin_wallets += 1; }
        let obj = json!({
            "w": addr,
            "n_tx": e.n_in + e.n_out,
            "n_in": e.n_in,
            "n_out": e.n_out,
            "n_tokens": e.tokens.len(),
            "n_chains": e.chains.len(),
            "depin_in": e.depin_in,
            "first": e.first,
            "last": e.last,
            "eth_native": eth.get(addr).copied().unwrap_or(0.0),
            "top": top3,
        });
        writeln!(w, "{}", obj).ok();
    }
    eprintln!("записано: {} гаманців -> {out}; з DePIN-купівлями: {depin_wallets}", ws.len());
}
