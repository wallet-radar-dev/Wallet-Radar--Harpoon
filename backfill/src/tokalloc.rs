//! tokalloc — масова перевірка позицій на АЛОКАЦІЮ: чи токен куплений на ринку, чи роздали.
//!
//! Навіщо: `tokdeep` робить окремий прохід по 23 ГБ на кожен токен — для двох десятків це
//! години. Тут усі токени перевіряються за ОДИН прохід.
//!
//! Ознаки алокації (їх і рахуємо на кожен токен):
//!   * одне зовнішнє джерело дає велику частку всього входу (`topSharePct`);
//!   * джерело — контракт-роздавач або нульова адреса `0x0000…0000` (прямий мінт);
//!   * сума від джерела «рівна» (5 000 000 000, 150 000 000) — ознака алокації, а не ринку;
//!   * мало холдерів при великій сумі.
//! Протилежне — ринкова купівля: джерел багато, найбільше дає одиниці відсотків, серед них
//! біржі (Binance тощо), холдерів десятки-сотні.
//!
//! Вихід: dataset/backfill/tokalloc.json + таблиця. Перевіряються ВСІ токени з ціною
//! (за net6k-positions.json), відсортовані за вартістю.
//! Args: ROOT [WALLET_FILE]. Env: CHAIN(1) MIN_USD(100000) TOP(40) SRC_KEEP(4000)
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, Write};

fn envn<T: std::str::FromStr>(k: &str, d: T) -> T {
    std::env::var(k).ok().and_then(|s| s.parse().ok()).unwrap_or(d)
}
fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let pat = format!("\"{key}\":\"");
    let i = line.find(&pat)? + pat.len();
    let rest = &line[i..];
    let j = rest.find('"')?;
    Some(&rest[..j])
}
/// «Рівне» число — ознака алокації: 5 000 000 000, 150 000 000, 20 000 000…
/// Ринок такими сумами не торгує, бо ціна й прослизання дають хвіст після коми.
fn is_round(x: f64) -> bool {
    if x < 1000.0 { return false }
    let mag = 10f64.powi((x.log10().floor() as i32) - 1);
    (x / mag).fract() < 1e-9
}

#[derive(Default)]
struct T {
    in_out: f64, in_in: f64, out_out: f64, out_in: f64,
    src: HashMap<String, f64>,
    holders: HashSet<u32>,
    from_zero: f64,
}

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let bf = format!("{root}/dataset/backfill");
    let chain: u32 = envn("CHAIN", 1);
    let min_usd: f64 = envn("MIN_USD", 100000.0);
    let top: usize = envn("TOP", 40);
    let src_keep: usize = envn("SRC_KEEP", 4000);
    let wfile = std::env::args().nth(2).unwrap_or_else(|| format!("{bf}/net-members.txt"));

    let members: HashSet<String> = BufReader::new(File::open(&wfile).expect("wallet file"))
        .lines().map_while(Result::ok)
        .map(|l| l.trim().to_lowercase()).filter(|l| l.len() == 42).collect();

    // які токени перевіряти — усі, де в групи є позиція дорожча за MIN_USD
    let pos: Value = serde_json::from_reader(BufReader::new(
        File::open(format!("{bf}/net6k-positions.json")).expect("спершу запусти net6k"))).unwrap();
    let mut want: HashMap<String, (String, f64)> = HashMap::new();
    for r in pos["rows"].as_array().cloned().unwrap_or_default() {
        let usd = r["usdNow"].as_f64().unwrap_or(0.0);
        if usd < min_usd { continue }
        let c = r["token"].as_str().unwrap_or("").to_lowercase();
        if !c.is_empty() { want.insert(c, (r["symbol"].as_str().unwrap_or("").to_string(), usd)); }
    }
    eprintln!("[tokalloc] токенів до перевірки: {} (позиція > ${min_usd})", want.len());

    let mut t: HashMap<String, T> = HashMap::new();
    let mut widx: HashMap<String, u32> = HashMap::new();
    let path = format!("{bf}/txfull-{chain}.jsonl");
    let rdr = BufReader::with_capacity(1 << 22, File::open(&path).expect("txfull"));
    let mut seen: HashSet<u64> = HashSet::new();
    let mut cur_w = String::new();
    let (mut n, mut hit) = (0u64, 0u64);

    for line in rdr.lines().map_while(Result::ok) {
        n += 1;
        if n % 20_000_000 == 0 { eprintln!("    ...{n} рядків, операцій {hit}"); }
        let Some(w) = field(&line, "w") else { continue };
        if !members.contains(w) { continue }
        if w != cur_w { cur_w = w.to_string(); seen.clear(); }
        let Some(tok) = field(&line, "tok") else { continue };
        if tok.is_empty() { continue }
        let tok = tok.to_lowercase();
        if !want.contains_key(&tok) { continue }
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        if v["k"].as_str() != Some("tok") { continue }

        let mut fp: u64 = 1469598103934665603;
        for s in [v["h"].as_str().unwrap_or(""), v["f"].as_str().unwrap_or(""),
                  v["t"].as_str().unwrap_or(""), v["v"].as_str().unwrap_or("")] {
            for b in s.as_bytes() { fp ^= *b as u64; fp = fp.wrapping_mul(1099511628211); }
        }
        if !seen.insert(fp) { continue }

        let frm = v["f"].as_str().unwrap_or("").to_lowercase();
        let to = v["t"].as_str().unwrap_or("").to_lowercase();
        let dec: i32 = v["dec"].as_str().and_then(|s| s.parse().ok()).unwrap_or(18);
        let raw: f64 = v["v"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0.0);
        let amt = raw / 10f64.powi(dec);
        if amt <= 0.0 { continue }
        hit += 1;

        let next = widx.len() as u32;
        let wi = *widx.entry(cur_w.clone()).or_insert(next);
        let e = t.entry(tok).or_default();
        if to == cur_w {
            e.holders.insert(wi);
            if members.contains(frm.as_str()) { e.in_in += amt } else {
                e.in_out += amt;
                if frm.starts_with(wr_store::addrs::zero()) { e.from_zero += amt }
                if e.src.len() < src_keep || e.src.contains_key(&frm) {
                    *e.src.entry(frm).or_insert(0.0) += amt;
                }
            }
        } else if frm == cur_w {
            if members.contains(to.as_str()) { e.out_in += amt } else { e.out_out += amt }
        }
    }
    eprintln!("[tokalloc] рядків {n}, операцій {hit}");

    let mut rows: Vec<Value> = Vec::new();
    for (c, e) in &t {
        let (sym, usd) = want.get(c).cloned().unwrap_or_default();
        let tot_in = e.in_out + e.in_in;
        if tot_in <= 0.0 { continue }
        let mut srcs: Vec<(&String, &f64)> = e.src.iter().collect();
        srcs.sort_by(|a, b| b.1.partial_cmp(a.1).unwrap());
        let (top_src, top_amt) = srcs.first().map(|(a, v)| ((*a).clone(), **v)).unwrap_or_default();
        let top_share = if e.in_out > 0.0 { 100.0 * top_amt / e.in_out } else { 0.0 };
        let round = is_round(top_amt);
        let minted = e.from_zero > 0.0;
        // вердикт: алокація, якщо одне джерело домінує, та ще й рівною сумою або прямим мінтом
        let verdict = if minted && e.from_zero / e.in_out.max(1e-9) > 0.3 { "МІНТ (пряма емісія)" }
            else if top_share >= 50.0 { if round { "АЛОКАЦІЯ (одне джерело, рівна сума)" } else { "ОДНЕ ДЖЕРЕЛО (>50%)" } }
            else if top_share >= 30.0 { if round { "ЙМОВІРНА АЛОКАЦІЯ (рівна сума)" } else { "концентроване джерело" } }
            else if e.holders.len() >= 100 && top_share < 15.0 { "РИНКОВА КУПІВЛЯ" }
            else { "розподілене" };
        rows.push(json!({
            "token": c, "symbol": sym, "usdNow": usd,
            "inOutside": e.in_out, "inInside": e.in_in,
            "outOutside": e.out_out, "outInside": e.out_in,
            "outsidePct": 100.0 * e.in_out / tot_in,
            "topSource": top_src, "topSourceAmt": top_amt, "topSharePct": top_share,
            "topIsRound": round, "fromZeroAddr": e.from_zero,
            "holders": e.holders.len(), "verdict": verdict,
            "sources": srcs.len(),
        }));
    }
    rows.sort_by(|a, b| b["usdNow"].as_f64().unwrap_or(0.0)
        .partial_cmp(&a["usdNow"].as_f64().unwrap_or(0.0)).unwrap());

    let outp = format!("{bf}/tokalloc.json");
    let _ = File::create(&outp).unwrap()
        .write_all(serde_json::to_string(&json!({"rows": rows})).unwrap().as_bytes());

    eprintln!("\n{:<10} {:>14} {:>7} {:>7} {:>6} {:>5}  {}", "ТОКЕН", "ПОЗИЦІЯ $", "ЗЗОВНІ", "ТОП-ДЖ", "РІВНА", "ХОЛД", "ВЕРДИКТ");
    for r in rows.iter().take(top) {
        eprintln!("{:<10} {:>14.0} {:>6.0}% {:>6.1}% {:>6} {:>5}  {}",
            r["symbol"].as_str().unwrap_or(""), r["usdNow"].as_f64().unwrap_or(0.0),
            r["outsidePct"].as_f64().unwrap_or(0.0), r["topSharePct"].as_f64().unwrap_or(0.0),
            if r["topIsRound"].as_bool().unwrap_or(false) { "так" } else { "-" },
            r["holders"].as_u64().unwrap_or(0), r["verdict"].as_str().unwrap_or(""));
    }
    let alloc: Vec<&Value> = rows.iter().filter(|r| {
        let v = r["verdict"].as_str().unwrap_or("");
        v.starts_with("АЛОКАЦІЯ") || v.starts_with("МІНТ") || v.starts_with("ЙМОВІРНА")
    }).collect();
    let sum: f64 = alloc.iter().filter_map(|r| r["usdNow"].as_f64()).sum();
    eprintln!("\n=== ПІДСУМОК ===");
    eprintln!("позицій усього: {} на ${:.0}", rows.len(),
        rows.iter().filter_map(|r| r["usdNow"].as_f64()).sum::<f64>());
    eprintln!("схожих на АЛОКАЦІЮ: {} на ${:.0}", alloc.len(), sum);
    for r in alloc.iter().take(15) {
        eprintln!("   {:<10} ${:>13.0} · джерело {} ({:.0}%)",
            r["symbol"].as_str().unwrap_or(""), r["usdNow"].as_f64().unwrap_or(0.0),
            r["topSource"].as_str().unwrap_or(""), r["topSharePct"].as_f64().unwrap_or(0.0));
    }
    eprintln!("\n[tokalloc] збережено {outp}");
}
