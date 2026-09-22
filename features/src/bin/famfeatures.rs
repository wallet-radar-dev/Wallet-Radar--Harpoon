//! Group-level feature layer: aggregate backfill transfers BY FAMILY (not wallet)
//! — the accurate unit, since intra-family transfers cancel out and a family's net
//! position is the real "smart money" position. Computes each family's SHARE OF
//! EMISSION per token (family net holding / total supply, from the supply collector)
//! and its sector exposure, then enriches families.json.
//!
//! In:  families-map.csv (wallet,family), transfers.csv, token-supply.json, families.json
//! Out: families.json (enriched: transfers, sectorMix, topEmission per family)

use std::collections::HashMap;
use std::io::{BufRead, BufReader};

use serde_json::Value;

const SECS: [&str; 5] = ["depin", "memecoin", "defi", "rwa", "other"];

fn is_stable(s: &str) -> bool {
    s.starts_with("USD") || s.ends_with("USD")
        || matches!(s, "DAI" | "GHO" | "FRAX" | "TUSD" | "PYUSD" | "USDE" | "USDS" | "RLUSD" | "CRVUSD" | "EURC" | "EURS" | "USDT0")
}
fn sector_map() -> HashMap<&'static str, usize> {
    let mut m = HashMap::new();
    let mut add = |l: &[&'static str], i: usize| for s in l { m.insert(*s, i); };
    add(&["RENDER","RNDR","FIL","AR","HNT","IOTX","AKT","THETA","TFUEL","GRT","POKT","LPT","DIMO","NOS","IO","AIOZ","JASMY","NKN","STORJ","GLM","FLUX","RSS3","PHA","HOPR","RLC","ANKR","TRAC","HOT","XYO","OCEAN","BLZ","MVL","AUDIO"], 0);
    add(&["SHIB","PEPE","DOGE","FLOKI","BONK","WIF","MOG","BRETT","POPCAT","MEME","ELON","TURBO","SPX","NEIRO","PNUT","FARTCOIN","BABYDOGE","LADYS","WOJAK","ANDY","DEGEN","APU","PORK","NPC","KISHU","KENDU","CAW","CATE","PEPECOIN","DOG","OSAK","BONE"], 1);
    add(&["UNI","AAVE","MKR","SKY","CRV","CVX","COMP","SNX","LDO","SUSHI","BAL","1INCH","DYDX","GMX","PENDLE","FXS","RPL","AERO","MORPHO","ENA","CAKE","SPELL","YFI","BADGER","ALCX","LQTY","STG","ZRX","KNC","WLFI","DEXE","ETHFI","OHM","SYRUP","EIGEN","QUICK","SOLV","XVS","GTC","PUFFER","POLS","SD","HFT","UMA","AEVO","REZ","SWELL","DPI","SPK","USUAL"], 2);
    add(&["ONDO","BUIDL","OUSG","USYC","USTBL","JAAA","USDY","XAUT","PAXG","KAU","KAG","CFG","GFI","VBILL","JTRSY","USTB","SAFO","EURSAFO","EUTBL","XAUM"], 3);
    m
}

fn main() {
    let root = std::env::var("APP_ROOT").unwrap_or_else(|_| "/opt/wallet-tracker".into());
    let smap = sector_map();

    // wallet -> family
    let mut fam_of: HashMap<String, String> = HashMap::new();
    if let Ok(f) = std::fs::File::open(format!("{root}/dataset/families-map.csv")) {
        for l in BufReader::new(f).lines().skip(1).map_while(Result::ok) {
            if let Some((w, fam)) = l.split_once(',') {
                fam_of.insert(w.to_lowercase(), fam.to_string());
            }
        }
    }
    // token-supply: contract -> adjusted supply
    let mut supply: HashMap<String, f64> = HashMap::new();
    let mut sym_of: HashMap<String, String> = HashMap::new();
    if let Ok(txt) = std::fs::read_to_string(format!("{root}/dataset/token-supply.json")) {
        if let Ok(Value::Object(o)) = serde_json::from_str::<Value>(&txt) {
            for (c, v) in o {
                let raw = v["supply"].as_str().unwrap_or("0").parse::<f64>().unwrap_or(0.0);
                let dec = v["decimals"].as_i64().unwrap_or(0);
                if raw > 0.0 {
                    supply.insert(c.to_lowercase(), raw / 10f64.powi(dec as i32));
                }
                if let Some(s) = v["symbol"].as_str() {
                    sym_of.insert(c.to_lowercase(), s.to_string());
                }
            }
        }
    }

    // aggregate transfers by family
    let mut n: HashMap<String, u64> = HashMap::new();
    let mut nin: HashMap<String, u64> = HashMap::new();
    let mut sect: HashMap<String, [u64; 5]> = HashMap::new();
    let mut net: HashMap<String, HashMap<String, f64>> = HashMap::new(); // family -> token -> net amount
    if let Ok(f) = std::fs::File::open(format!("{root}/dataset/backfill/transfers.csv")) {
        for l in BufReader::with_capacity(1 << 20, f).lines().map_while(Result::ok) {
            let p: Vec<&str> = l.splitn(9, ',').collect();
            if p.len() < 9 || p[0] == "wallet" { continue; }
            let Some(fam) = fam_of.get(&p[0].to_lowercase()) else { continue };
            *n.entry(fam.clone()).or_default() += 1;
            if p[2] == "i" { *nin.entry(fam.clone()).or_default() += 1; }
            let sym = p[5].to_uppercase();
            let si = if is_stable(&sym) { 4 } else { *smap.get(sym.as_str()).unwrap_or(&4) };
            sect.entry(fam.clone()).or_insert([0; 5])[si] += 1;
            let amt: f64 = p[7].parse().unwrap_or(0.0);
            let signed = if p[2] == "i" { amt } else { -amt };
            *net.entry(fam.clone()).or_default().entry(p[4].to_lowercase()).or_default() += signed;
        }
    }

    // enrich families.json
    let fp = format!("{root}/dataset/families.json");
    let mut doc: Value = serde_json::from_str(&std::fs::read_to_string(&fp).expect("families.json")).unwrap();
    let mut enriched = 0;
    if let Some(arr) = doc["families"].as_array_mut() {
        for fam in arr.iter_mut() {
            let key = fam["root"].as_str().unwrap_or("").to_string();
            let tot = *n.get(&key).unwrap_or(&0);
            fam["transfers"] = Value::from(tot);
            if tot == 0 { continue; }
            enriched += 1;
            let ins = *nin.get(&key).unwrap_or(&0);
            fam["inRatio"] = Value::from(((ins as f64 / tot as f64) * 100.0).round() / 100.0);
            // Phase: net incoming vs outgoing = accumulating into / distributing out of the family.
            let ir = ins as f64 / tot as f64;
            fam["phase"] = Value::from(if ir >= 0.55 { "accumulating" } else if ir <= 0.45 { "distributing" } else { "neutral" });
            // sector mix
            if let Some(sc) = sect.get(&key) {
                let mix: Vec<Value> = (0..5)
                    .filter(|&i| sc[i] > 0)
                    .map(|i| serde_json::json!({"sector": SECS[i], "pct": ((sc[i] as f64 / tot as f64) * 100.0).round()}))
                    .collect();
                fam["sectorMix"] = Value::Array(mix);
                // Dominant narrative sector (exclude 'other').
                if let Some(di) = (0..4).filter(|&i| sc[i] > 0).max_by_key(|&i| sc[i]) {
                    fam["dominantSector"] = Value::from(SECS[di]);
                }
            }
            // share of emission per token (net holding / total supply)
            if let Some(tn) = net.get(&key) {
                let mut em: Vec<(String, f64)> = tn
                    .iter()
                    .filter_map(|(c, &amt)| {
                        let sup = *supply.get(c)?;
                        if sup <= 0.0 || amt <= 0.0 { return None; }
                        // Flow-net proxy for holding; clamp at 100% (a family cannot hold
                        // more than a token's whole supply — >100% just means heavy churn).
                        let share = ((amt / sup) * 100.0).min(100.0);
                        let sym = sym_of.get(c).cloned().unwrap_or_else(|| c[..8.min(c.len())].to_string());
                        Some((sym, share))
                    })
                    .collect();
                em.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
                let top: Vec<Value> = em.iter().take(8)
                    .map(|(s, sh)| serde_json::json!({"symbol": s, "emissionPct": (sh * 100.0).round() / 100.0}))
                    .collect();
                if !top.is_empty() {
                    fam["topEmission"] = Value::Array(top);
                }
            }
        }
    }
    std::fs::write(&fp, serde_json::to_string(&doc).unwrap()).unwrap();
    eprintln!("[famfeatures] enriched {} families with transfer data (of {} total); supply for {} tokens", enriched, doc["familiesCount"], supply.len());
}
