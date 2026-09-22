//! tokdeep — глибокий розріз по ОДНОМУ токену: хто з групи його тримає, коли зайшов,
//! і головне — прийшов токен ЗЗОВНІ (купівля на ринку) чи ЗСЕРЕДИНИ (роздача від своїх).
//!
//! Навіщо: у групі 6864 гаманців UNI дає 80% усього нереалізованого прибутку (16% емісії,
//! середній строк утримання 473 дні). Питання «це 271 незалежний холдер чи один центр, що
//! роздав по гаманцях» вирішує, чи є позиція узгодженою.
//!
//! Розрізняємо три джерела надходження:
//!   * ЗСЕРЕДИНИ — відправник теж член групи (внутрішній розподіл, не купівля);
//!   * ЗЗОВНІ    — відправник поза групою (біржа, DEX, будь-хто);
//!   * і симетрично для виходу.
//! Якщо більшість обсягу зайшла зсередини — «271 холдер» це одна позиція, розкладена по
//! адресах, а не 271 рішення.
//!
//! Вихід: dataset/backfill/tokdeep-<symbol>.json + звіт.
//! Args: ROOT [WALLET_FILE]. Env: TOKEN(контракт, обов'язково) CHAIN(1) TOP(40)
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

#[derive(Default, Clone)]
struct H {
    bal: f64,
    in_out: f64,     // надійшло ЗЗОВНІ
    in_in: f64,      // надійшло ЗСЕРЕДИНИ
    out_out: f64,    // пішло НАЗОВНІ
    out_in: f64,     // пішло ВСЕРЕДИНУ
    first: u64,
    last: u64,
    n: u32,
}

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let bf = format!("{root}/dataset/backfill");
    let chain: u32 = envn("CHAIN", 1);
    let top: usize = envn("TOP", 40);
    let token = std::env::var("TOKEN").expect("вкажи TOKEN=<контракт>").to_lowercase();
    let wfile = std::env::args().nth(2).unwrap_or_else(|| format!("{bf}/net-members.txt"));

    let members: HashSet<String> = BufReader::new(File::open(&wfile).expect("wallet file"))
        .lines().map_while(Result::ok)
        .map(|l| l.trim().to_lowercase())
        .filter(|l| l.len() == 42)
        .collect();

    let cur: Value = serde_json::from_reader(BufReader::new(
        File::open(format!("{bf}/token-prices.json")).expect("token-prices.json"))).unwrap();
    let px = cur["tokens"][&token]["usd"].as_f64().unwrap_or(0.0);
    let sym = cur["tokens"][&token]["symbol"].as_str().unwrap_or("?").to_string();
    eprintln!("[tokdeep] {sym} ({token}) · ціна ${px} · членів {}", members.len());

    let mut h: HashMap<String, H> = HashMap::new();
    // з ким саме торгували ззовні — покаже, чи джерело спільне
    let mut ext_src: HashMap<String, f64> = HashMap::new();
    let mut ext_dst: HashMap<String, f64> = HashMap::new();

    let path = format!("{bf}/txfull-{chain}.jsonl");
    let rdr = BufReader::with_capacity(1 << 22, File::open(&path).expect("txfull"));
    let mut seen: HashSet<u64> = HashSet::new();
    let mut cur_w = String::new();
    let (mut n, mut hit) = (0u64, 0u64);

    for line in rdr.lines().map_while(Result::ok) {
        n += 1;
        if n % 20_000_000 == 0 { eprintln!("    ...{n} рядків, знайдено {hit}"); }
        // швидкий відсів: рядок навіть не згадує наш контракт
        if !line.contains(&token) { continue }
        let Some(w) = field(&line, "w") else { continue };
        if !members.contains(w) { continue }
        if w != cur_w { cur_w = w.to_string(); seen.clear(); }
        let Ok(t) = serde_json::from_str::<Value>(&line) else { continue };
        if t["k"].as_str() != Some("tok") { continue }
        if t["tok"].as_str().map(|s| s.to_lowercase()).as_deref() != Some(token.as_str()) { continue }

        let mut fp: u64 = 1469598103934665603;
        for s in [t["h"].as_str().unwrap_or(""), t["f"].as_str().unwrap_or(""),
                  t["t"].as_str().unwrap_or(""), t["v"].as_str().unwrap_or("")] {
            for b in s.as_bytes() { fp ^= *b as u64; fp = fp.wrapping_mul(1099511628211); }
        }
        if !seen.insert(fp) { continue }

        let frm = t["f"].as_str().unwrap_or("").to_lowercase();
        let to = t["t"].as_str().unwrap_or("").to_lowercase();
        let ts = t["ts"].as_u64().unwrap_or(0);
        let dec: i32 = t["dec"].as_str().and_then(|s| s.parse().ok()).unwrap_or(18);
        let raw: f64 = t["v"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0.0);
        let amt = raw / 10f64.powi(dec);
        if amt <= 0.0 { continue }
        hit += 1;

        let e = h.entry(cur_w.clone()).or_default();
        e.n += 1;
        if e.first == 0 || ts < e.first { e.first = ts }
        if ts > e.last { e.last = ts }
        if to == cur_w {
            e.bal += amt;
            if members.contains(frm.as_str()) { e.in_in += amt } else {
                e.in_out += amt;
                *ext_src.entry(frm.clone()).or_insert(0.0) += amt;
            }
        } else if frm == cur_w {
            e.bal -= amt;
            if members.contains(to.as_str()) { e.out_in += amt } else {
                e.out_out += amt;
                *ext_dst.entry(to.clone()).or_insert(0.0) += amt;
            }
        }
    }
    eprintln!("[tokdeep] рядків {n}, операцій з токеном {hit}");

    let mut hs: Vec<(&String, &H)> = h.iter().filter(|(_, v)| v.bal > 1e-9).collect();
    hs.sort_by(|a, b| b.1.bal.partial_cmp(&a.1.bal).unwrap());

    let tot_bal: f64 = hs.iter().map(|(_, v)| v.bal).sum();
    let tot_in_out: f64 = h.values().map(|v| v.in_out).sum();
    let tot_in_in: f64 = h.values().map(|v| v.in_in).sum();
    let tot_out_out: f64 = h.values().map(|v| v.out_out).sum();
    let tot_out_in: f64 = h.values().map(|v| v.out_in).sum();

    eprintln!("\n=== {sym}: ЗВІДКИ ВЗЯВСЯ ===");
    eprintln!("надійшло ЗЗОВНІ:    {:>18.0} ({:.1}%)", tot_in_out,
        100.0 * tot_in_out / (tot_in_out + tot_in_in).max(1e-9));
    eprintln!("надійшло ЗСЕРЕДИНИ: {:>18.0} ({:.1}%)", tot_in_in,
        100.0 * tot_in_in / (tot_in_out + tot_in_in).max(1e-9));
    eprintln!("пішло НАЗОВНІ:      {:>18.0}", tot_out_out);
    eprintln!("пішло ВСЕРЕДИНУ:    {:>18.0}", tot_out_in);
    eprintln!("тримають зараз: {:.0} на {} адресах (${:.0})", tot_bal, hs.len(), tot_bal * px);

    let mut es: Vec<(&String, &f64)> = ext_src.iter().collect();
    es.sort_by(|a, b| b.1.partial_cmp(a.1).unwrap());
    eprintln!("\n=== ЗВІДКИ ЗАХОДИВ ІЗЗОВНІ (топ-10 джерел) ===");
    for (a, v) in es.iter().take(10) {
        eprintln!("  {} {:>16.0} ({:.1}% усього зовнішнього входу)", a, v,
            100.0 * **v / tot_in_out.max(1e-9));
    }
    let mut ed: Vec<(&String, &f64)> = ext_dst.iter().collect();
    ed.sort_by(|a, b| b.1.partial_cmp(a.1).unwrap());
    eprintln!("\n=== КУДИ ВИХОДИВ НАЗОВНІ (топ-10) ===");
    for (a, v) in ed.iter().take(10) {
        eprintln!("  {} {:>16.0}", a, v);
    }

    eprintln!("\n=== ТОП-{top} ХОЛДЕРІВ ===");
    for (a, v) in hs.iter().take(top) {
        let src = if v.in_in > v.in_out { "зсередини" } else { "ззовні" };
        eprintln!("  {} {:>14.0} (${:>12.0}) · {} · перший {} · оп {}",
            a, v.bal, v.bal * px, src,
            chrono_ymd(v.first), v.n);
    }

    let out = json!({
        "token": token, "symbol": sym, "price": px,
        "holders": hs.len(), "heldTotal": tot_bal, "heldUsd": tot_bal * px,
        "inFromOutside": tot_in_out, "inFromInside": tot_in_in,
        "outToOutside": tot_out_out, "outToInside": tot_out_in,
        "topExternalSources": es.iter().take(20).map(|(a, v)| json!({"addr": a, "amt": v})).collect::<Vec<_>>(),
        "topExternalSinks": ed.iter().take(20).map(|(a, v)| json!({"addr": a, "amt": v})).collect::<Vec<_>>(),
        "topHolders": hs.iter().take(200).map(|(a, v)| json!({
            "addr": a, "bal": v.bal, "usd": v.bal * px,
            "inOutside": v.in_out, "inInside": v.in_in,
            "outOutside": v.out_out, "outInside": v.out_in,
            "first": v.first, "last": v.last, "ops": v.n })).collect::<Vec<_>>(),
    });
    let outp = format!("{bf}/tokdeep-{}.json", sym.to_lowercase());
    let _ = File::create(&outp).unwrap().write_all(serde_json::to_string(&out).unwrap().as_bytes());
    eprintln!("\n[tokdeep] збережено {outp}");
}

/// Дата з unix-часу без зовнішніх залежностей — лише рік-місяць-день, більшого тут не треба.
fn chrono_ymd(ts: u64) -> String {
    if ts == 0 { return "?".into() }
    let days = ts / 86400;
    let (mut y, mut d) = (1970i64, days as i64);
    loop {
        let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
        let n = if leap { 366 } else { 365 };
        if d < n { break }
        d -= n; y += 1;
    }
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let ml = [31, if leap {29} else {28}, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut m = 0;
    while m < 12 && d >= ml[m] { d -= ml[m]; m += 1 }
    format!("{y}-{:02}-{:02}", m + 1, d + 1)
}
