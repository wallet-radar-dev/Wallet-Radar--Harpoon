//! txindex — перетворює сирий лог транзакцій на БАЗУ з миттєвим доступом по гаманцю.
//!
//! `txfull-<chain>.jsonl` росте до десятків ГБ. Без індексу будь-яке питання «покажи історію цього
//! гаманця» означає повний скан файлу. Але записи одного гаманця пишуться суцільним блоком
//! (усі його рядки скидаються за один захват м'ютекса), тож достатньо запам'ятати зміщення й
//! довжину блоку — і читання стає одним seek замість скану.
//!
//! Режими:
//!   без аргументу гаманця — БУДУЄ індекс: txfull-<chain>.idx.json
//!   з аргументом         — ЧИТАЄ історію одного гаманця через індекс (seek, без скану)
//! Вихід індексу: {wallet:{off,len,n,first,last,kinds}}
//! Args: ROOT [WALLET]. Env: CHAIN(1)
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let want = std::env::args().nth(2).map(|s| s.trim().to_lowercase());
    let chain: u32 = std::env::var("CHAIN").ok().and_then(|s| s.parse().ok()).unwrap_or(1);
    let bf = format!("{root}/dataset/backfill");
    let data = format!("{bf}/txfull-{chain}.jsonl");
    let idxp = format!("{bf}/txfull-{chain}.idx.json");

    // --- ЧИТАННЯ через індекс ---
    if let Some(w) = want {
        let idx: Value = serde_json::from_reader(BufReader::new(File::open(&idxp).expect("нема індексу — спершу побудуй"))).unwrap();
        let Some(e) = idx["wallets"].get(&w) else { eprintln!("[txindex] гаманця {w} нема в базі"); return };
        let (off, len) = (e["off"].as_u64().unwrap_or(0), e["len"].as_u64().unwrap_or(0) as usize);
        let mut f = File::open(&data).expect("нема даних");
        f.seek(SeekFrom::Start(off)).ok();
        let mut buf = vec![0u8; len];
        f.read_exact(&mut buf).ok();
        print!("{}", String::from_utf8_lossy(&buf));
        eprintln!("[txindex] {w}: {} транзакцій, {} … {}", e["n"], e["first"], e["last"]);
        return;
    }

    // --- ПОБУДОВА індексу ---
    let f = File::open(&data).expect("нема txfull-<chain>.jsonl");
    let total = f.metadata().map(|m| m.len()).unwrap_or(0);
    let mut rd = BufReader::with_capacity(1 << 22, f);
    let mut off: u64 = 0;
    let mut line = String::new();
    // wallet -> (off, len, n, first_ts, last_ts, kinds)
    let mut idx: HashMap<String, (u64, u64, u64, u64, u64, std::collections::HashSet<String>)> = HashMap::new();
    let mut rows = 0u64;
    loop {
        line.clear();
        let n = rd.read_line(&mut line).unwrap_or(0);
        if n == 0 { break; }
        rows += 1;
        if let Ok(v) = serde_json::from_str::<Value>(&line) {
            let w = v["w"].as_str().unwrap_or("").to_string();
            let ts = v["ts"].as_u64().unwrap_or(0);
            let k = v["k"].as_str().unwrap_or("").to_string();
            let e = idx.entry(w).or_insert((off, 0, 0, u64::MAX, 0, Default::default()));
            // блок гаманця суцільний: зміщення фіксуємо перше, довжину нарощуємо
            e.1 = (off + n as u64) - e.0;
            e.2 += 1;
            if ts > 0 { if ts < e.3 { e.3 = ts } if ts > e.4 { e.4 = ts } }
            e.5.insert(k);
        }
        off += n as u64;
        if rows % 2_000_000 == 0 { eprintln!("[txindex] {rows} рядків, {:.1}% файлу", 100.0 * off as f64 / total.max(1) as f64); }
    }
    let out: serde_json::Map<String, Value> = idx.iter().map(|(w, (o, l, n, fs, ls, ks))| {
        let mut kv: Vec<&String> = ks.iter().collect(); kv.sort();
        (w.clone(), json!({"off": o, "len": l, "n": n,
            "first": if *fs == u64::MAX { 0 } else { *fs }, "last": ls,
            "kinds": kv}))
    }).collect();
    let tot_tx: u64 = idx.values().map(|v| v.2).sum();
    std::fs::write(&idxp, serde_json::to_string(&json!({
        "chain": chain, "file": data, "wallets": out, "rows": rows, "bytes": total})).unwrap()).ok();
    eprintln!("[txindex] база {chain}: {} гаманців · {} транзакцій · {:.2} ГБ · індекс → {idxp}",
        idx.len(), tot_tx, total as f64 / 1073741824.0);
}
