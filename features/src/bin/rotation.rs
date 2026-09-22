//! Temporal narrative rotation from the full backfill: monthly NET whale flow
//! (incoming - outgoing, non-stable transfers) per sector, across 8 years of
//! history. Positive = whales accumulating that narrative that month.
//!
//! In:  dataset/backfill/transfers.csv  (wallet,chain,dir,ts,token,symbol,cp,amount,hash)
//! Out: dataset/sector-rotation.json  {months:[], sectors:{sec:{net:[],buy:[],sell:[]}}}

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};

const SECS: [&str; 5] = ["depin", "memecoin", "defi", "rwa", "other"];

fn is_stable(s: &str) -> bool {
    s.starts_with("USD")
        || s.ends_with("USD")
        || matches!(s, "DAI" | "GHO" | "FRAX" | "TUSD" | "PYUSD" | "USDE" | "USDS" | "RLUSD" | "CRVUSD" | "EURC" | "EURS" | "USDT0")
}

fn sector_map() -> HashMap<&'static str, usize> {
    let mut m = HashMap::new();
    let mut add = |list: &[&'static str], idx: usize| for s in list { m.insert(*s, idx); };
    add(&["RENDER","RNDR","FIL","AR","HNT","IOTX","AKT","THETA","TFUEL","GRT","POKT","LPT","DIMO","NOS","IO","AIOZ","PEAQ","JASMY","MOBILE","IOT","NKN","STORJ","GLM","FLUX","SHDW","RSS3","PHA","HOPR","RLC","ANKR","TRAC","HOT","XYO","OCEAN","BLZ","MVL","AUDIO"], 0);
    add(&["SHIB","PEPE","DOGE","FLOKI","BONK","WIF","MOG","BRETT","POPCAT","MEME","ELON","TURBO","SPX","PONKE","MEW","NEIRO","GIGA","PNUT","GOAT","FARTCOIN","BABYDOGE","LADYS","WOJAK","ANDY","TOSHI","DEGEN","APU","PORK","NPC","KISHU","KENDU","PEIPEI","CAW","CATE","KEKIUS","MANYU","OMIKAMI","PEPECOIN","DOG","OSAK","BONE"], 1);
    add(&["UNI","AAVE","MKR","SKY","CRV","CVX","COMP","SNX","LDO","SUSHI","BAL","1INCH","DYDX","GMX","PENDLE","FXS","RPL","AERO","VELO","MORPHO","ENA","CAKE","JOE","SPELL","YFI","BADGER","ALCX","LQTY","RUNE","OSMO","GNS","SILO","EUL","FLUID","COW","STG","RDNT","ZRX","KNC","WLFI","DEXE","LIT","ETHFI","OHM","SYRUP","FF","EIGEN","QUICK","HEGIC","LEND","SOLV","BIFI","MCB","AUTO","XVS","BEL","NXM","TORN","SDEX","SDT","ANGLE","DODO","DRV","TEMPLE","SYN","FARM","BNT","MLN","GEAR","GTC","PUFFER","POLS","SD","HFT","UMA","DDX","OGN","AEVO","REZ","SWELL","KRL","CPOOL","RESOLV","DPI","SPK","USUAL"], 2);
    add(&["ONDO","BUIDL","OUSG","USYC","USTBL","JAAA","FDIT","USDY","XAUT","PAXG","KAU","KAG","POLYX","RIO","CFG","MPL","TRU","GFI","OM","PROPS","VBILL","JTRSY","THBILL","BENJI","TBILL","USTB","WTGXX","SAFO","EURSAFO","BDX","EUTBL","XAUM","VNXAU","XAGM","XGZ","MTBILL","DGLD","GLDX"], 3);
    m
}

fn main() {
    let root = std::env::var("APP_ROOT").unwrap_or_else(|_| "/opt/wallet-tracker".into());
    let inp = format!("{root}/dataset/backfill/transfers.csv");
    let outp = format!("{root}/dataset/sector-rotation.json");
    let smap = sector_map();

    let f = std::fs::File::open(&inp).expect("open transfers.csv");
    let rdr = BufReader::with_capacity(1 << 20, f);
    // month -> [ [buy,sell] ; 5 sectors ]
    let mut agg: HashMap<String, [[i64; 2]; 5]> = HashMap::new();
    let mut rows = 0u64;

    for line in rdr.lines() {
        let l = match line { Ok(l) => l, Err(_) => continue };
        let p: Vec<&str> = l.splitn(9, ',').collect();
        if p.len() < 9 || p[0] == "wallet" { continue; }
        let (dir, ts, symbol) = (p[2], p[3], p[5]);
        if ts.len() < 7 { continue; }
        let sym = symbol.to_uppercase();
        if is_stable(&sym) { continue; } // narratives only
        let si = *smap.get(sym.as_str()).unwrap_or(&4); // 4 = other
        rows += 1;
        let month = ts[..7].to_string(); // YYYY-MM
        let e = agg.entry(month).or_insert([[0i64; 2]; 5]);
        match dir {
            "i" => e[si][0] += 1, // buy / incoming
            "o" => e[si][1] += 1, // sell / outgoing
            _ => {}
        }
    }

    let mut months: Vec<String> = agg.keys().cloned().collect();
    months.sort();
    // keep from 2020 on (earlier is thin/noisy)
    months.retain(|m| m.as_str() >= "2020-01");

    let mut out = String::from("{\"months\":[");
    out.push_str(&months.iter().map(|m| format!("\"{m}\"")).collect::<Vec<_>>().join(","));
    out.push_str("],\"sectors\":{");
    let mut secparts: Vec<String> = Vec::new();
    for (si, sec) in SECS.iter().enumerate() {
        let buy: Vec<i64> = months.iter().map(|m| agg[m][si][0]).collect();
        let sell: Vec<i64> = months.iter().map(|m| agg[m][si][1]).collect();
        let net: Vec<i64> = buy.iter().zip(&sell).map(|(b, s)| b - s).collect();
        let arr = |v: &[i64]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(",");
        secparts.push(format!(
            "\"{sec}\":{{\"net\":[{}],\"buy\":[{}],\"sell\":[{}]}}",
            arr(&net), arr(&buy), arr(&sell)
        ));
    }
    out.push_str(&secparts.join(","));
    out.push_str("}}");

    let mut w = std::io::BufWriter::new(std::fs::File::create(&outp).expect("create out"));
    w.write_all(out.as_bytes()).unwrap();
    w.flush().unwrap();
    eprintln!("[rotation] {} narrative rows -> {} months -> {}", rows, months.len(), outp);
}
