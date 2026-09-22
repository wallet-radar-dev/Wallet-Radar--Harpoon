//! Глибокий аналіз ETH-ротації-когорти (китова смуга 100-20k ETH з eth-native-net.json).
//! (1) Портфельні під-архетипи: k-means по фічах [ln eth, ln n_in, n_tokens, depin%, stable%, scr, cro].
//! (2) Shared-token "дески": рідкісні токени (тримають 2..=DESK_MAX когорти) дають ребра
//!     між власниками з вагою IDF=ln(N/holders); union-find -> компоненти (ознака координації).
//! Вихід: dataset/backfill/cohort-analysis.json + звіт. Env: BAND_LO=100 BAND_HI=20000 K=5
//! DESK_MAX=25 EDGE_MIN=2.5.

use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader};

const DEPIN: &[&str] = &["ankr","rlc","grt","pha","audio","woo","link","render","rndr",
    "fil","hnt","iotx","theta","akt","dimo","nosana","io","gomining","peaq"];
const STABLE: &[&str] = &["usdc","usdt","usdt0","usdc.e","dai","usde","tusd","busd"];

fn envf(k: &str, d: f64) -> f64 { std::env::var(k).ok().and_then(|s| s.parse().ok()).unwrap_or(d) }
fn envu(k: &str, d: usize) -> usize { std::env::var(k).ok().and_then(|s| s.parse().ok()).unwrap_or(d) }

struct Uf { p: Vec<usize> }
impl Uf {
    fn new(n: usize) -> Self { Uf { p: (0..n).collect() } }
    fn find(&mut self, x: usize) -> usize { if self.p[x]!=x { let r=self.find(self.p[x]); self.p[x]=r; } self.p[x] }
    fn union(&mut self, a: usize, b: usize) { let (ra,rb)=(self.find(a),self.find(b)); if ra!=rb { self.p[ra]=rb; } }
}

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let d = format!("{root}/dataset/backfill");
    let (lo, hi) = (envf("BAND_LO", 100.0), envf("BAND_HI", 20000.0));
    let k = envu("K", 5);
    let desk_max = envu("DESK_MAX", 25);
    let edge_min = envf("EDGE_MIN", 2.5);
    let dep: HashSet<&str> = DEPIN.iter().copied().collect();
    let stb: HashSet<&str> = STABLE.iter().copied().collect();

    let raw: HashMap<String, f64> = serde_json::from_reader(
        BufReader::new(File::open(format!("{d}/eth-native-net.json")).expect("eth json"))
    ).expect("parse");
    let eth_all: HashMap<String, f64> = raw.into_iter().map(|(a,v)|(a.to_lowercase(),v)).collect();

    // Реальні (ціновані) токени — щоб відсіяти скам-ейрдропи (paws|t.me, *.cfd, ...).
    let mut priced: HashSet<String> = HashSet::new();
    if let Ok(rd) = fs::read_dir(format!("{root}/dataset/nextaction/prices")) {
        for ent in rd.flatten() {
            let fname = ent.file_name().to_string_lossy().to_string();
            if let Some(stem) = fname.strip_suffix(".json") {
                if let Some((c, a)) = stem.split_once('_') {
                    priced.insert(format!("{c}\t{a}"));
                }
            }
        }
    }
    eprintln!("[cohort] цінових (реальних) токенів: {}", priced.len());

    // Набір гаманців: MEMBERS_FILE -> довільний набір (∩ активні); UNIVERSE=1 -> усі активні;
    // інакше ETH-смуга.
    let universe = std::env::var("UNIVERSE").is_ok();
    let members_file = std::env::var("MEMBERS_FILE").ok();
    let min_act: u32 = envu("MIN_ACT", 5) as u32;
    let band: HashMap<String, f64>;
    if universe || members_file.is_some() {
        let fileset: Option<HashSet<String>> = members_file.as_ref().map(|p| {
            std::fs::read_to_string(p).unwrap_or_default().lines()
                .map(|l| l.trim().to_lowercase()).filter(|l| l.len() == 42 && l.starts_with("0x")).collect()
        });
        let mut act: HashMap<String, u32> = HashMap::new();
        let f = BufReader::new(File::open(format!("{d}/transfers.csv")).expect("transfers.csv"));
        for line in f.lines().map_while(Result::ok) {
            let mut l = line.splitn(6, ',');
            let w = l.next().unwrap_or("");
            let chain = l.next().unwrap_or("");
            let dir = l.next().unwrap_or("?");
            let _ts = l.next();
            let token = l.next().unwrap_or("");
            if dir != "i" || w == wr_store::addrs::zero() { continue; }
            let wl = w.to_lowercase();
            if let Some(fs) = &fileset { if !fs.contains(&wl) { continue; } }
            if !priced.contains(&format!("{chain}\t{token}")) { continue; }
            *act.entry(wl).or_insert(0) += 1;
        }
        band = act.into_iter().filter(|(_, c)| *c >= min_act)
            .map(|(w, _)| { let e = eth_all.get(&w).copied().unwrap_or(0.0); (w, e) }).collect();
        eprintln!("[cohort] {}: {} гаманців з >= {} priced-receipts",
            if fileset.is_some() { "MEMBERS" } else { "UNIVERSE" }, band.len(), min_act);
    } else {
        band = eth_all.iter().filter(|(_, v)| **v >= lo && **v <= hi)
            .map(|(k, v)| (k.clone(), *v)).collect();
        eprintln!("[cohort] смуга {lo}-{hi} ETH: {} китів", band.len());
    }
    let mut ids: Vec<String> = band.keys().cloned().collect();
    ids.sort(); // детермінізм: HashMap-порядок випадковий щопроцес
    let idx: HashMap<&String, usize> = ids.iter().enumerate().map(|(i,a)|(a,i)).collect();
    let n = ids.len();

    // портфелі: per-wallet набір токен-адрес (dir=i), лічильники тегів
    let mut toks: Vec<HashSet<String>> = vec![HashSet::new(); n];
    let mut n_in = vec![0u32; n];
    let mut depin_in = vec![0u32; n];
    let mut stable_in = vec![0u32; n];
    let mut has_scr = vec![false; n];
    let mut has_cro = vec![false; n];
    let mut tok_sym: HashMap<String, String> = HashMap::new(); // token-addr -> symbol
    let f = BufReader::new(File::open(format!("{d}/transfers.csv")).expect("transfers.csv"));
    for line in f.lines().map_while(Result::ok) {
        let mut left = line.splitn(6, ',');
        let w = left.next().unwrap_or("");
        let chain = left.next().unwrap_or("");
        let dir = left.next().unwrap_or("?");
        let _ts = left.next();
        let token = left.next().unwrap_or("");
        if dir != "i" { continue; }
        let wl = w.to_lowercase();
        let Some(&i) = idx.get(&wl) else { continue };
        let key = format!("{chain}\t{token}");
        if !priced.contains(&key) { continue; } // тільки реальні токени
        let sym = line.split(',').nth(5).unwrap_or("").trim().to_lowercase();
        n_in[i] += 1;
        toks[i].insert(key.clone());
        tok_sym.entry(key).or_insert(sym.clone());
        if dep.contains(sym.as_str()) { depin_in[i] += 1; }
        if stb.contains(sym.as_str()) { stable_in[i] += 1; }
        if sym == "scr" { has_scr[i] = true; }
        if sym == "cro" { has_cro[i] = true; }
    }

    // ---- (1) k-means під-архетипи ----
    let feats: Vec<[f64;7]> = (0..n).map(|i| {
        let ni = n_in[i].max(1) as f64;
        [ (band[&ids[i]].max(0.0)+1.0).ln(), ((n_in[i]+1) as f64).ln(), toks[i].len() as f64,
          depin_in[i] as f64/ni, stable_in[i] as f64/ni,
          has_scr[i] as u8 as f64, has_cro[i] as u8 as f64 ]
    }).collect();
    let mut mean=[0.0;7]; let mut sd=[0.0;7];
    for f in &feats { for j in 0..7 { mean[j]+=f[j]; } }
    for m in &mut mean { *m/=n as f64; }
    for f in &feats { for j in 0..7 { sd[j]+=(f[j]-mean[j]).powi(2); } }
    for s in &mut sd { *s=(*s/n as f64).sqrt().max(1e-9); }
    let z: Vec<[f64;7]> = feats.iter().map(|f| { let mut o=[0.0;7]; for j in 0..7 { o[j]=(f[j]-mean[j])/sd[j]; } o }).collect();
    // maximin (k-means++ детермінований) init: 1-й центр z[0], далі — найдальша точка
    let mut cent: Vec<[f64;7]> = vec![z[0]];
    while cent.len() < k {
        let (mut best, mut bd) = (0usize, -1.0);
        for i in 0..n {
            let mut md = f64::MAX;
            for c in &cent { let mut dd=0.0; for j in 0..7 { dd+=(z[i][j]-c[j]).powi(2); } if dd<md { md=dd; } }
            if md > bd { bd = md; best = i; }
        }
        cent.push(z[best]);
    }
    let mut asg = vec![0usize; n];
    for _ in 0..50 {
        for i in 0..n {
            let (mut b,mut bd)=(0,f64::MAX);
            for (c,ct) in cent.iter().enumerate() { let mut dd=0.0; for j in 0..7 { dd+=(z[i][j]-ct[j]).powi(2);} if dd<bd {bd=dd;b=c;} }
            asg[i]=b;
        }
        let mut sum=vec![[0.0;7];k]; let mut cnt=vec![0u64;k];
        for i in 0..n { cnt[asg[i]]+=1; for j in 0..7 { sum[asg[i]][j]+=z[i][j]; } }
        for c in 0..k { if cnt[c]>0 { for j in 0..7 { cent[c][j]=sum[c][j]/cnt[c] as f64; } } }
    }
    let mut clusters = Vec::new();
    println!("=== ПІД-АРХЕТИПИ (k={k}) ===");
    for c in 0..k {
        let mem: Vec<usize> = (0..n).filter(|&i| asg[i]==c).collect();
        if mem.is_empty() { continue; }
        let sz = mem.len();
        let avg_eth: f64 = mem.iter().map(|&i| band[&ids[i]]).sum::<f64>()/sz as f64;
        let avg_tok: f64 = mem.iter().map(|&i| toks[i].len() as f64).sum::<f64>()/sz as f64;
        let dep_sh = mem.iter().filter(|&&i| depin_in[i]>0).count() as f64/sz as f64;
        let scr = mem.iter().filter(|&&i| has_scr[i]).count();
        let cro = mem.iter().filter(|&&i| has_cro[i]).count();
        // топ спільні символи в кластері
        let mut sc: HashMap<String,u32> = HashMap::new();
        for &i in &mem { for t in &toks[i] { if let Some(s)=tok_sym.get(t){ if !s.is_empty(){*sc.entry(s.clone()).or_insert(0)+=1;} } } }
        let mut sv: Vec<(String,u32)> = sc.into_iter().collect();
        sv.sort_by(|a,b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        let top: Vec<String> = sv.iter().take(8).map(|(s,c)| format!("{s}:{c}")).collect();
        let mut ex: Vec<(&String,f64)> = mem.iter().map(|&i|(&ids[i],band[&ids[i]])).collect();
        ex.sort_by(|a,b| b.1.partial_cmp(&a.1).unwrap());
        let examples: Vec<String> = ex.iter().take(3).map(|(a,_)|(*a).clone()).collect();
        println!("  C{c}: n={sz} avgETH={avg_eth:.0} avgTok={avg_tok:.0} DePIN={:.0}% SCR={scr} CRO={cro}", dep_sh*100.0);
        println!("       топ: {}", top.join(" "));
        let members_full: Vec<String> = mem.iter().map(|&i| ids[i].clone()).collect();
        clusters.push(json!({"c":c,"size":sz,"avg_eth":avg_eth.round(),"avg_tokens":avg_tok.round(),
            "depin_share":(dep_sh*100.0).round(),"scr":scr,"cro":cro,"top_symbols":top,"examples":examples,"members":members_full}));
    }

    // ---- (2) shared-token дески ----
    // token-addr -> список власників-когорти
    let mut holders: HashMap<&String, Vec<usize>> = HashMap::new();
    for i in 0..n { for t in &toks[i] { holders.entry(t).or_default().push(i); } }
    let mut pair: HashMap<(usize,usize), f64> = HashMap::new();
    for (t, hs) in &holders {
        let h = hs.len();
        if h < 2 || h > desk_max { continue; } // рідкісні, але спільні
        let idf = (n as f64 / h as f64).ln();
        let sym = tok_sym.get(*t).cloned().unwrap_or_default();
        // пропустити стейбли/сміття-символи як неінформативні (URL-скам)
        if stb.contains(sym.as_str()) || sym.contains('.') || sym.is_empty() { continue; }
        for a in 0..h { for b in (a+1)..h {
            let key = (hs[a].min(hs[b]), hs[a].max(hs[b]));
            *pair.entry(key).or_insert(0.0) += idf;
        }}
    }
    let mut uf = Uf::new(n);
    let mut edges = 0;
    for (&(a,b), &wgt) in &pair { if wgt >= edge_min { uf.union(a,b); edges+=1; } }
    let mut comp: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..n { let r = uf.find(i); comp.entry(r).or_default().push(i); }
    let mut desks: Vec<Vec<usize>> = comp.into_values().filter(|v| v.len()>=3).collect();
    desks.sort_by(|a,b| b.len().cmp(&a.len()));
    println!("\n=== SHARED-TOKEN ДЕСКИ (ребра>={edge_min} IDF, рідкісні токени 2..={desk_max}) ===");
    println!("ребер: {edges}; десків (>=3): {}", desks.len());
    let mut desks_json = Vec::new();
    for (di, mem) in desks.iter().enumerate().take(12) {
        // спільні токени в межах деска
        let mut sc: HashMap<String,u32> = HashMap::new();
        for &i in mem { for t in &toks[i] {
            let h = holders.get(t).map(|v| v.len()).unwrap_or(0);
            if h>=2 && h<=desk_max { if let Some(s)=tok_sym.get(t){ if !s.is_empty() && !s.contains('.') && !stb.contains(s.as_str()){*sc.entry(s.clone()).or_insert(0)+=1;} } }
        }}
        let mut sv: Vec<(String,u32)> = sc.into_iter().filter(|(_,c)| *c>=2).collect();
        sv.sort_by(|a,b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        let shared: Vec<String> = sv.iter().take(10).map(|(s,c)| format!("{s}:{c}")).collect();
        let avg_eth: f64 = mem.iter().map(|&i| band[&ids[i]]).sum::<f64>()/mem.len() as f64;
        let ex: Vec<String> = mem.iter().take(4).map(|&i| ids[i].clone()).collect();
        println!("  D{di}: n={} avgETH={:.0} спільні: {}", mem.len(), avg_eth, shared.join(" "));
        desks_json.push(json!({"d":di,"size":mem.len(),"avg_eth":avg_eth.round(),"shared_tokens":shared,"members":ex}));
    }

    // ---- (3) label-propagation community detection (суб-розбиття щільного ядра) ----
    // Зважений граф з ребер >= edge_min. Кожен вузол бере мітку з макс. сумою ваг сусідів
    // (детермінований tie-break: менша мітка). Фіксований порядок вузлів, 30 ітерацій.
    let mut adj: HashMap<usize, Vec<(usize, f64)>> = HashMap::new();
    for (&(a, b), &wgt) in &pair {
        if wgt < edge_min { continue; }
        adj.entry(a).or_default().push((b, wgt));
        adj.entry(b).or_default().push((a, wgt));
    }
    let mut label: Vec<usize> = (0..n).collect();
    for _ in 0..30 {
        let mut changed = false;
        for i in 0..n {
            let Some(nb) = adj.get(&i) else { continue };
            if nb.is_empty() { continue; }
            let mut score: HashMap<usize, f64> = HashMap::new();
            for &(j, w) in nb { *score.entry(label[j]).or_insert(0.0) += w; }
            // макс за вагою, tie -> менша мітка
            let best = score.iter().max_by(|a, b|
                a.1.partial_cmp(b.1).unwrap().then(b.0.cmp(a.0))).map(|(l, _)| *l).unwrap();
            if best != label[i] { label[i] = best; changed = true; }
        }
        if !changed { break; }
    }
    let mut comm: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..n { if adj.contains_key(&i) { comm.entry(label[i]).or_default().push(i); } }
    let mut communities: Vec<Vec<usize>> = comm.into_values().filter(|v| v.len() >= 3).collect();
    communities.sort_by(|a, b| b.len().cmp(&a.len()));
    println!("\n=== LABEL-PROP СПІЛЬНОТИ (суб-розбиття ядра) ===");
    println!("спільнот (>=3): {}", communities.len());
    let mut comm_json = Vec::new();
    for (ci, mem) in communities.iter().enumerate().take(12) {
        let mut sc: HashMap<String, u32> = HashMap::new();
        for &i in mem { for t in &toks[i] {
            let h = holders.get(t).map(|v| v.len()).unwrap_or(0);
            if h >= 2 && h <= desk_max { if let Some(s) = tok_sym.get(t) {
                if !s.is_empty() && !s.contains('.') && !stb.contains(s.as_str()) { *sc.entry(s.clone()).or_insert(0) += 1; }
            }}
        }}
        let mut sv: Vec<(String, u32)> = sc.into_iter().filter(|(_, c)| *c >= 2).collect();
        sv.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        let shared: Vec<String> = sv.iter().take(10).map(|(s, c)| format!("{s}:{c}")).collect();
        let avg_eth: f64 = mem.iter().map(|&i| band[&ids[i]]).sum::<f64>() / mem.len() as f64;
        let all_mem: Vec<String> = mem.iter().map(|&i| ids[i].clone()).collect();
        let dep_sh = mem.iter().filter(|&&i| depin_in[i] > 0).count() as f64 / mem.len() as f64;
        println!("  K{ci}: n={} avgETH={:.0} DePIN={:.0}% спільні: {}", mem.len(), avg_eth, dep_sh * 100.0, shared.join(" "));
        comm_json.push(json!({"k":ci,"size":mem.len(),"avg_eth":avg_eth.round(),
            "depin_share":(dep_sh*100.0).round(),"shared_tokens":shared,"members":all_mem}));
    }

    let band_members: Vec<String> = ids.clone();
    let outname = if members_file.is_some() { "meta-subnets.json" }
        else if universe { "groups-universe.json" } else { "cohort-analysis.json" };
    serde_json::to_writer_pretty(File::create(format!("{d}/{outname}")).unwrap(),
        &json!({"band":n,"universe":universe,"band_lo":lo,"band_hi":hi,"band_members":band_members,"archetypes":clusters,
                "desks_total":desks.len(),"desks":desks_json,
                "communities_total":communities.len(),"communities":comm_json})).unwrap();
    eprintln!("збережено: {outname}");
}
