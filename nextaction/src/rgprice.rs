//! rgprice (підхід Б) — ончейн-ціни токенів зі свопів у БЕКФІЛІ, без мережі/RPC.
//! У тому самому (wallet, tx_hash) одна нога = реальний СТЕЙБЛ (за контрактом, не символом —
//! бо є homoglyph-скам USDC/USDT), інша = токен, у протилежному напрямі → ціна = stable/token.
//! amount у transfers — ЛЮДСЬКИЙ, тож ціна пряма. Вихід: dataset/backfill/onchain-prices.json {token: usd}.
//! 2 проходи (memory-light): П1 — стейбл-ноги per (wallet,tx); П2 — токен-ноги → медіана ціни/токен.

use serde_json::json;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Write};

// Реальні стейбл-контракти (mainnet+L2) = $1. Перелік у базі, тег `rgprice:stables`.
fn stables_list() -> Vec<String> {
    let c = wr_store::open().expect("rgprice: без сховища немає переліку стейблів");
    let mut q = c
        .prepare("SELECT address FROM wallet_tags WHERE tag='rgprice:stables'")
        .expect("запит тегу");
    let v: Vec<String> =
        q.query_map([], |r| r.get(0)).expect("читання").filter_map(|r| r.ok()).collect();
    assert!(!v.is_empty(), "rgprice: тег rgprice:stables порожній — ціни поїдуть");
    v
}


fn parse(line: &str) -> Option<(String, char, String, f64, String)> {
    // -> (wallet, dir, token, amount(human), tx_hash)
    let mut it = line.splitn(6, ',');
    let w = it.next()?.to_lowercase();
    let _chain = it.next()?;
    let dir = it.next()?;
    let _ts = it.next()?;
    let tok = it.next()?.to_lowercase();
    let d = match dir { "i" => 'i', "o" => 'o', _ => return None };
    let mut rit = line.rsplitn(4, ',');
    let tx = rit.next()?.to_string();
    let amt: f64 = rit.next()?.parse().ok()?;
    Some((w, d, tok, amt, tx))
}

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let tf = format!("{root}/dataset/backfill/transfers.csv");
    let sl = stables_list();
    let stables: std::collections::HashSet<&str> = sl.iter().map(String::as_str).collect();
    let open = || BufReader::with_capacity(1 << 20, File::open(&tf).expect("transfers.csv"));

    // П1: стейбл-нога per (wallet,tx) -> (usd, dir) ; беремо найбільшу
    let mut stx: HashMap<(String, String), (f64, char)> = HashMap::new();
    for line in open().lines().map_while(Result::ok) {
        let Some((w, d, tok, amt, tx)) = parse(&line) else { continue };
        if amt <= 0.0 || !stables.contains(tok.as_str()) { continue; }
        let e = stx.entry((w, tx)).or_insert((0.0, d));
        if amt > e.0 { *e = (amt, d); }
    }
    eprintln!("[rgprice] П1: стейбл-свопів (wallet,tx): {}", stx.len());

    // П2: токен-нога у тих самих tx, протилежний напрям -> ціна
    let mut prices: HashMap<String, Vec<f64>> = HashMap::new();
    for line in open().lines().map_while(Result::ok) {
        let Some((w, d, tok, amt, tx)) = parse(&line) else { continue };
        if amt <= 0.0 || stables.contains(tok.as_str()) { continue; }
        if let Some(&(usd, sd)) = stx.get(&(w.clone(), tx)) {
            if d != sd && usd > 0.0 && amt > 0.0 {
                let p = usd / amt;
                if p.is_finite() && p > 0.0 { prices.entry(tok).or_default().push(p); }
            }
        }
    }
    eprintln!("[rgprice] П2: токенів зі свопною ціною: {}", prices.len());

    // медіана ціни per токен (≥3 свопи для стійкості)
    let mut out = serde_json::Map::new();
    let mut kept = 0;
    for (tok, mut v) in prices {
        if v.len() < 3 { continue; }
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let med = v[v.len() / 2];
        out.insert(tok, json!({"usd": med, "swaps": v.len()}));
        kept += 1;
    }
    let path = format!("{root}/dataset/backfill/onchain-prices.json");
    let mut f = File::create(&path).unwrap();
    f.write_all(serde_json::to_string(&json!(out)).unwrap().as_bytes()).ok();
    eprintln!("[rgprice] збережено {} токен-цін (≥3 свопи) -> onchain-prices.json", kept);
}
