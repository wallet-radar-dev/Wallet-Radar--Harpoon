//! Крок 1 навчання next-action: категоризатор контрагентів + замір розподілу «дій».
//!
//! Читає dataset/backfill/transfers.csv (лише dir=o — дії, які гаманець зробив САМ,
//! а не спам, що йому прилетів) + dataset/labels-cache.json, категоризує counterparty
//! (cex/dex/bridge/defi/eoa) і рахує розподіл дій-міток майбутньої моделі.
//!
//! СТОП-ЧЕК: якщо transfer(eoa) домінує (>90%), supervised-ціль треба переглянути —
//! сигналу для прогнозу «наступної дії» замало (те саме, що ми бачили на китах).
//!
//! Парсинг CSV: counterparty/amount/tx_hash — останні 3 поля, без ком, тож беремо
//! rsplit(nth 2); dir — 3-тє поле зліва, теж без ком. symbol (може мати коми, хоч і
//! санітизується) нас не зачіпає.

use serde_json::Value;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Cat {
    Cex,
    Dex,
    Bridge,
    Defi,
    Eoa,
}
impl Cat {
    fn action(self) -> &'static str {
        match self {
            Cat::Cex => "sell_cex",
            Cat::Dex => "swap_dex",
            Cat::Bridge => "bridge",
            Cat::Defi => "defi",
            Cat::Eoa => "transfer",
        }
    }
}

/// Порядок як у category_of: cex -> bridge -> dex -> defi -> eoa (перше співпадіння виграє).
fn categorize(name: &str, labels: &[String]) -> Cat {
    let mut hay = name.to_lowercase();
    for l in labels {
        hay.push(' ');
        hay.push_str(&l.to_lowercase());
    }
    let any = |ws: &[&str]| ws.iter().any(|w| hay.contains(w));
    if any(&[
        "binance", "coinbase", "okx", "kraken", "bybit", "huobi", "kucoin", "gate.io", "gateio",
        "bitfinex", "gemini", "bitstamp", "mexc", "bitget", "upbit", "htx", "crypto.com",
        "cryptocom", "crypto-com", "exchange",
    ]) {
        Cat::Cex
    } else if any(&[
        "bridge", "wormhole", "portal", "across", "stargate", "synapse", "orbiter", "debridge",
        "layerzero", "celer", "relay",
    ]) {
        Cat::Bridge
    } else if any(&[
        "uniswap", "sushiswap", "1inch", "paraswap", "kyber", "cowswap", "aggregator", "pancake",
        "odos", "bancor", "0x-protocol", "-swap", "dex",
    ]) {
        Cat::Dex
    } else if any(&[
        "aave", "compound", "maker", "curve", "balancer", "lido", "rocketpool", "staking", "yield",
        "lending", "vault", "protocol", "pool", "dao", "defi",
    ]) {
        Cat::Defi
    } else {
        Cat::Eoa
    }
}

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());

    // labels-cache.json -> address(lower) -> Cat
    let lab: Value = serde_json::from_reader(BufReader::new(
        File::open(format!("{root}/dataset/labels-cache.json")).expect("labels-cache.json"),
    ))
    .expect("parse labels");
    let mut cat_of: HashMap<String, Cat> = HashMap::new();
    if let Some(obj) = lab.as_object() {
        for (addr, v) in obj {
            let name = v["name"].as_str().unwrap_or("");
            let labels: Vec<String> = v["labels"]
                .as_array()
                .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
                .unwrap_or_default();
            cat_of.insert(addr.to_lowercase(), categorize(name, &labels));
        }
    }
    eprintln!("[nextaction] міток завантажено: {}", cat_of.len());

    // transfers.csv (стрімінг; файл може дописуватись backfill-ом — читаємо до поточного EOF)
    let f = BufReader::new(
        File::open(format!("{root}/dataset/backfill/transfers.csv")).expect("transfers.csv"),
    );
    let mut tally: HashMap<&str, u64> = HashMap::new();
    let (mut total_out, mut labeled, mut lines, mut d_in, mut d_out, mut d_q) =
        (0u64, 0u64, 0u64, 0u64, 0u64, 0u64);

    for line in f.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };
        lines += 1;
        let dir = line.split(',').nth(2).unwrap_or("?");
        match dir {
            "i" => d_in += 1,
            "o" => d_out += 1,
            "?" => d_q += 1,
            _ => continue, // "dir" (header) та все інше
        }
        if dir != "o" {
            continue;
        }
        let cp = line.rsplit(',').nth(2).unwrap_or("");
        total_out += 1;
        let cat = match cat_of.get(cp) {
            Some(c) => {
                labeled += 1;
                *c
            }
            None => Cat::Eoa,
        };
        *tally.entry(cat.action()).or_insert(0) += 1;
    }

    let pct = |n: u64, d: u64| 100.0 * n as f64 / d.max(1) as f64;
    println!("\n=== ЗАМІР РОЗПОДІЛУ ДІЙ (backfill, поточний зріз) ===");
    println!("рядків прочитано:  {lines}");
    println!("напрям: in={d_in}  out={d_out}  ?={d_q}");
    println!(
        "покриття міток на вихідних: {labeled}/{total_out} ({:.1}%)",
        pct(labeled, total_out)
    );
    println!("\nмітка (наступна дія):");
    for a in ["sell_cex", "swap_dex", "bridge", "defi", "transfer"] {
        let n = tally.get(a).copied().unwrap_or(0);
        println!("  {a:9} {n:>10}  ({:.1}%)", pct(n, total_out));
    }
    let transfer_share = pct(tally.get("transfer").copied().unwrap_or(0), total_out);
    println!(
        "\nСТОП-ЧЕК: transfer(eoa) = {:.1}%  ->  {}",
        transfer_share,
        if transfer_share > 90.0 {
            "сигналу замало, ціль переглянути"
        } else {
            "є з чим працювати"
        }
    );
}
