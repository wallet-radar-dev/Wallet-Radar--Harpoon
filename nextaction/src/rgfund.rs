//! rgfund (#2) — граф спільного ФІНАНСУВАННЯ. Перший вхідний нативний ETH (symbol=="ETH") кожного
//! члена CAP-груп → фандер. Фандери, що живлять ≥K членів/кілька груп = ОПЕРАТОРИ (доказ координації:
//! 200 гаманців з одного джерела = один оператор, не 200 інвесторів). Анотує групи topFunder/funderShare
//! + пише rgfund.json (оператори, що зшивають групи). Env: MINK(3). Лише бекфіл.
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, Write};

fn is_junk(a: &str) -> bool { a.is_empty() || wr_store::addrs::is_junk_addr(a) }

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let mink: usize = std::env::var("MINK").ok().and_then(|s| s.parse().ok()).unwrap_or(3);
    let cgp = format!("{root}/dataset/capital-groups.json");
    let lbl: serde_json::Map<String, Value> = serde_json::from_reader(BufReader::new(File::open(format!("{root}/dataset/labels-cache.json")).unwrap())).unwrap_or_default();
    let lab = |a: &str| lbl.get(a).and_then(|x| x["name"].as_str()).map(|s| s.to_string()).unwrap_or_else(|| format!("{}…{}", &a[..6.min(a.len())], &a[a.len().saturating_sub(4)..]));
    let is_exch = |a: &str| lbl.get(a).and_then(|x| x["name"].as_str()).map(|n| { let l = n.to_lowercase(); ["binance","crypto.com","okx","gate","coinbase","kraken","bybit","kucoin","bitget","circle","bridge","galaxy","falconx","wintermute","cumberland","tether","bitfinex","huobi","mexc","bitstamp","gemini","korbit","upbit"].iter().any(|c| l.contains(c)) }).unwrap_or(false);

    // CAP-групи: addr -> {групи}, gid -> symbol
    let cg: Value = serde_json::from_reader(BufReader::new(File::open(&cgp).unwrap())).unwrap();
    let groups = cg["groups"].as_array().cloned().unwrap_or_default();
    let mut gidx: Vec<usize> = Vec::new(); // індекси CAP-груп у масиві
    let mut gsym: Vec<String> = Vec::new();
    let mut addr2g: HashMap<String, Vec<usize>> = HashMap::new();
    let mut members: HashSet<String> = HashSet::new();
    for (i, g) in groups.iter().enumerate() {
        if !g["id"].as_str().unwrap_or("").starts_with("CAP:") { continue; }
        let li = gidx.len(); gidx.push(i);
        gsym.push(g["commonAssets"].as_array().and_then(|a| a.first()).and_then(|s| s.as_str()).unwrap_or("").to_string());
        for m in g["members"].as_array().unwrap_or(&vec![]) { if let Some(a) = m["address"].as_str() { let a = a.to_lowercase(); addr2g.entry(a.clone()).or_default().push(li); members.insert(a); } }
    }
    eprintln!("[rgfund] CAP-груп: {}, унікальних членів: {}", gidx.len(), members.len());

    // перший вхідний ETH-фандер per член — з eth-funders.jsonl (краул Etherscan, ~100% покриття);
    // фолбек на розріджений ETH з transfers, якщо файлу нема.
    let mut eth: HashMap<String, (String, String)> = HashMap::new(); // wallet -> (ts, funder)
    let fpath = format!("{root}/dataset/backfill/eth-funders.jsonl");
    if let Ok(f) = File::open(&fpath) {
        for line in BufReader::new(f).lines().map_while(Result::ok) {
            if let Ok(v) = serde_json::from_str::<Value>(&line) {
                let w = v["w"].as_str().unwrap_or("").to_lowercase();
                if !members.contains(&w) { continue; }
                if let Some(fu) = v["funder"].as_str() { if !is_junk(fu) { eth.insert(w, (v["ts"].as_u64().unwrap_or(0).to_string(), fu.to_lowercase())); } }
            }
        }
        eprintln!("[rgfund] джерело фандерів: eth-funders.jsonl ({} членів з фандером)", eth.len());
    } else {
        for line in BufReader::with_capacity(1 << 20, File::open(format!("{root}/dataset/backfill/transfers.csv")).unwrap()).lines().map_while(Result::ok) {
            let mut it = line.splitn(6, ','); let w = it.next().unwrap_or(""); let _c = it.next(); let dir = it.next().unwrap_or(""); let ts = it.next().unwrap_or(""); let _tok = it.next(); let rest = it.next().unwrap_or("");
            if dir != "i" { continue; }
            let wl = w.to_lowercase(); if !members.contains(&wl) { continue; }
            if rest.split(',').next().unwrap_or("") != "ETH" { continue; }
            let mut rit = line.rsplitn(4, ','); let _h = rit.next(); let amt: f64 = rit.next().unwrap_or("0").parse().unwrap_or(0.0); let cp = rit.next().unwrap_or("").to_lowercase();
            if amt <= 0.0 || is_junk(&cp) { continue; }
            let e = eth.entry(wl).or_insert((String::from("~"), String::new()));
            if ts < e.0.as_str() { *e = (ts.to_string(), cp); }
        }
        eprintln!("[rgfund] джерело фандерів: transfers.csv fallback ({} членів)", eth.len());
    }

    // фандер -> члени, фандер -> групи
    let mut fmem: HashMap<String, HashSet<String>> = HashMap::new();
    let mut fgrp: HashMap<String, HashSet<usize>> = HashMap::new();
    for (w, (_, cp)) in &eth { fmem.entry(cp.clone()).or_default().insert(w.clone()); for &g in &addr2g[w] { fgrp.entry(cp.clone()).or_default().insert(g); } }

    // ОПЕРАТОРИ: фандери, що живлять ≥MINK членів. Розділяємо біржові (шум) від EOA (справжні оператори).
    let mut ops: Vec<(String, usize, usize, bool)> = fmem.iter().filter(|(_, m)| m.len() >= mink).map(|(f, m)| (f.clone(), m.len(), fgrp.get(f).map(|s| s.len()).unwrap_or(0), is_exch(f))).collect();
    ops.sort_by(|a, b| b.1.cmp(&a.1));
    let eoa_ops: Vec<&(String, usize, usize, bool)> = ops.iter().filter(|o| !o.3).collect();
    eprintln!("[rgfund] фандерів-операторів (≥{} членів): {} (з них EOA-неміченим: {}, біржових: {})", mink, ops.len(), eoa_ops.len(), ops.len() - eoa_ops.len());
    let multi_grp = eoa_ops.iter().filter(|o| o.2 >= 2).count();
    eprintln!("[rgfund] EOA-операторів, що зшивають ≥2 групи: {} (доказ єдиної мережі на рівні фінансування)", multi_grp);
    eprintln!("[rgfund] --- ТОП EOA-оператори (немічені, не біржі) ---");
    for (f, nm, ng, _) in eoa_ops.iter().take(20) { eprintln!("   {:<40} → {} членів у {} групах", lab(f), nm, ng); }
    eprintln!("[rgfund] --- біржові джерела (шум, для контексту) ---");
    for (f, nm, ng, ex) in ops.iter().filter(|o| o.3).take(6) { let _ = ex; eprintln!("   {:<40} → {} членів у {} групах", lab(f), nm, ng); }

    // покриття: скільки членів мають ВІДОМОГО фандера, і топ-оператор
    let with_funder = eth.len();
    eprintln!("[rgfund] членів з відомим ETH-фандером: {}/{} ({:.0}%)", with_funder, members.len(), with_funder as f64 / members.len().max(1) as f64 * 100.0);

    // на кожну групу: топ-фандер і частка членів від нього
    let cgp2 = cgp.clone();
    let mut cg2: Value = serde_json::from_reader(BufReader::new(File::open(&cgp2).unwrap())).unwrap();
    if let Ok(cur) = std::fs::read(&cgp2) { File::create(format!("{cgp2}.bak-fund")).and_then(|mut f| f.write_all(&cur)).ok(); }
    // gid -> (funder counts)
    let mut g_funders: Vec<HashMap<String, usize>> = vec![HashMap::new(); gidx.len()];
    for (w, (_, cp)) in &eth { for &g in &addr2g[w] { *g_funders[g].entry(cp.clone()).or_insert(0) += 1; } }
    let mut annotated = 0;
    if let Some(arr) = cg2["groups"].as_array_mut() {
        for (li, &ai) in gidx.iter().enumerate() {
            let fu = &g_funders[li];
            let n_mem = groups[ai]["members"].as_array().map(|a| a.len()).unwrap_or(0).max(1);
            let (top_f, top_c) = fu.iter().max_by_key(|(_, c)| **c).map(|(f, c)| (f.clone(), *c)).unwrap_or((String::new(), 0));
            let share = top_c as f64 / n_mem as f64;
            if let Some(o) = arr[ai].as_object_mut() {
                o.insert("topFunder".into(), json!(if top_f.is_empty() { Value::Null } else { json!(lab(&top_f)) }));
                o.insert("topFunderShare".into(), json!((share * 1e3).round() / 1e3));
                o.insert("distinctFunders".into(), json!(fu.len()));
                annotated += 1;
            }
        }
    }
    nextaction::safewrite::save_json_or_die("rgfund", &cgp2, &cg2);
    eprintln!("[rgfund] анотовано груп фандером: {}", annotated);

    let ops_json: Vec<Value> = ops.iter().take(80).map(|(f, nm, ng, ex)| json!({"funder": f, "label": lab(f), "kind": if *ex { "exchange" } else { "eoa" }, "members": nm, "groups": ng, "assets": fgrp.get(f).map(|s| { let mut v: Vec<&str> = s.iter().map(|&g| gsym[g].as_str()).collect(); v.sort(); v }).unwrap_or_default()})).collect();
    nextaction::safewrite::save_json_or_die("rgfund", &format!("{root}/dataset/backfill/rgfund.json"), &json!({"operators": ops_json, "withFunder": with_funder, "members": members.len(), "multiGroupOps": multi_grp}));
}
