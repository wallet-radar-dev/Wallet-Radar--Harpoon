//! Пошук КООРДИНОВАНИХ ГРУП великого капіталу з backfill.
//!
//! Ідея: гаманці однієї «руки»/групи роблять ТІ САМІ рухи (chain, token, напрям) у ТОЙ САМИЙ
//! день (≤24г), навіть якщо МІЖ СОБОЮ НЕ ПЕРЕТИНАЛИСЬ (нема прямих транзакцій). Це поведінковий
//! слід, а не on-chain лінк (на відміну від прод coordgroup=спільні фандери).
//!
//! Алгоритм:
//!  1) унікальні події (wallet, chain\token, day, dir);
//!  2) ко-рух: у кожній події (chain\token,day,dir) гаманці, що діяли разом → пари;
//!     вага пари = IDF токена (рідкісний токен co-move = сильний сигнал; популярні ігнор);
//!  3) нормалізація score по активності (щоб надактивні не домінували);
//!  4) ребра (score≥поріг і ≥MIN_SHARED спільних подій) → union-find компоненти → ГРУПИ.
//! Вихід: dataset/nextaction/wallet-groups.json.
//!
//! Env: POP(300) MAX_GROUP(40) MIN_SHARED(3) EDGE_MIN(0.06) TOP(40).

use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use chrono::DateTime;


fn env_usize(k: &str, d: usize) -> usize {
    std::env::var(k).ok().and_then(|s| s.parse().ok()).unwrap_or(d)
}
fn env_i64(k: &str, d: i64) -> i64 {
    std::env::var(k).ok().and_then(|s| s.parse().ok()).unwrap_or(d)
}
fn env_f64(k: &str, d: f64) -> f64 {
    std::env::var(k).ok().and_then(|s| s.parse().ok()).unwrap_or(d)
}
fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

// union-find
struct Uf {
    p: Vec<u32>,
}
impl Uf {
    fn new(n: usize) -> Self {
        Uf { p: (0..n as u32).collect() }
    }
    fn find(&mut self, x: u32) -> u32 {
        let mut r = x;
        while self.p[r as usize] != r {
            r = self.p[r as usize];
        }
        let mut c = x;
        while self.p[c as usize] != r {
            let n = self.p[c as usize];
            self.p[c as usize] = r;
            c = n;
        }
        r
    }
    fn union(&mut self, a: u32, b: u32) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.p[ra as usize] = rb;
        }
    }
}

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let pop = env_usize("POP", 40);
    let max_group = env_usize("MAX_GROUP", 40);
    let min_shared = env_usize("MIN_SHARED", 6) as u32;
    let edge_min = env_f64("EDGE_MIN", 0.4);
    let top = env_usize("TOP", 40);
    let bucket_hours: i64 = env_i64("BUCKET_HOURS", 12).max(1);

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
    eprintln!("[coord] цінових токенів {}", priced.len());

    // Pass 1: унікальні події
    let f = BufReader::new(
        File::open(format!("{root}/dataset/backfill/transfers.csv")).expect("transfers.csv"),
    );
    let mut wid: HashMap<String, u32> = HashMap::new();
    let mut waddr: Vec<String> = Vec::new();
    let mut activity: Vec<u32> = Vec::new();
    // token(chain\ttoken) -> distinct wallets (IDF/популярність)
    let mut token_wallets: HashMap<String, HashSet<u32>> = HashMap::new();
    // подія (chain\ttoken\tday\tdir) -> гаманці
    let mut events: HashMap<String, HashSet<u32>> = HashMap::new();
    // token-symbol для звіту
    let mut symbol: HashMap<String, String> = HashMap::new();

    let mut rows = 0u64;
    for line in f.lines() {
        let Ok(line) = line else { continue };
        let mut left = line.splitn(6, ',');
        let wallet = left.next().unwrap_or("");
        if wallet == "wallet" || wallet == wr_store::addrs::zero() {
            continue;
        }
        let chain = left.next().unwrap_or("");
        let dir = left.next().unwrap_or("?");
        let ts = left.next().unwrap_or("");
        let token = left.next().unwrap_or("");
        let rest = left.next().unwrap_or(""); // symbol,counterparty,...
        if (dir != "i" && dir != "o") || token.is_empty() {
            continue;
        }
        let id = *wid.entry(wallet.to_string()).or_insert_with(|| {
            waddr.push(wallet.to_string());
            activity.push(0);
            (waddr.len() - 1) as u32
        });
        let tkey = format!("{chain}\t{token}");
        if !priced.contains(&tkey) { continue; }
        let bucket = match DateTime::parse_from_rfc3339(ts) { Ok(dt) => dt.timestamp() / (bucket_hours * 3600), Err(_) => continue };
        let ekey = format!("{tkey}\t{bucket}\t{dir}");
        if events.entry(ekey).or_default().insert(id) {
            token_wallets.entry(tkey.clone()).or_default().insert(id);
            activity[id as usize] += 1;
            symbol.entry(tkey).or_insert_with(|| rest.split(',').next().unwrap_or("").to_string());
        }
        rows += 1;
    }
    let nw = waddr.len();
    eprintln!("[coord] рядків {rows}, гаманців {nw}, унікальних подій {}", events.len());

    // Pass 2: пари з ко-рухом (IDF-зважені), лише розрізняльні токени й події
    let mut pair: HashMap<(u32, u32), (f64, u32)> = HashMap::new();
    // спільні токени пари (для звіту) — обмежено топ
    let mut pair_tok: HashMap<(u32, u32), HashMap<String, u32>> = HashMap::new();
    let mut skipped_pop = 0u64;
    for (ekey, ws) in &events {
        let n = ws.len();
        if n < 2 || n > max_group {
            continue;
        }
        let tkey = {
            let mut it = ekey.splitn(3, '\t');
            format!("{}\t{}", it.next().unwrap_or(""), it.next().unwrap_or(""))
        };
        let tw = token_wallets.get(&tkey).map(|s| s.len()).unwrap_or(0);
        if tw == 0 || tw > pop {
            skipped_pop += 1;
            continue;
        }
        let idf = (nw as f64 / tw as f64).ln();
        let v: Vec<u32> = ws.iter().copied().collect();
        for i in 0..v.len() {
            for j in (i + 1)..v.len() {
                let (a, b) = if v[i] < v[j] { (v[i], v[j]) } else { (v[j], v[i]) };
                let e = pair.entry((a, b)).or_insert((0.0, 0));
                e.0 += idf;
                e.1 += 1;
                *pair_tok.entry((a, b)).or_default().entry(tkey.clone()).or_insert(0) += 1;
            }
        }
    }
    eprintln!("[coord] пар {}, пропущено популярних подій {skipped_pop}", pair.len());

    // Ребра: нормалізований score, ≥MIN_SHARED спільних подій
    let mut edges: Vec<(u32, u32, f64, u32)> = Vec::new();
    for (&(a, b), &(score, cnt)) in &pair {
        if cnt < min_shared {
            continue;
        }
        let norm = score / ((activity[a as usize] as f64) * (activity[b as usize] as f64)).sqrt();
        if norm >= edge_min {
            edges.push((a, b, norm, cnt));
        }
    }
    edges.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap());
    eprintln!("[coord] ребер (сильна координація) {}", edges.len());

    // Union-find → компоненти (групи)
    let mut uf = Uf::new(nw);
    for &(a, b, _, _) in &edges {
        uf.union(a, b);
    }
    let mut comp: HashMap<u32, Vec<u32>> = HashMap::new();
    for w in 0..nw as u32 {
        // лише гаманці, що потрапили в якесь ребро
        let r = uf.find(w);
        if r != w || uf.p[w as usize] != w {
            // включаємо тільки якщо має ребро (нижче фільтруємо за розміром)
        }
        comp.entry(uf.find(w)).or_default().push(w);
    }
    // залишаємо лише групи 2..=MAX_MEMBERS, де є координація
    let member_in_edge: HashSet<u32> = edges.iter().flat_map(|&(a, b, _, _)| [a, b]).collect();
    let mut groups: Vec<Value> = Vec::new();
    for (_root, mems) in &comp {
        let mem: Vec<u32> = mems.iter().copied().filter(|w| member_in_edge.contains(w)).collect();
        if mem.len() < 2 {
            continue;
        }
        // сила групи: сума нормо-score внутрішніх ребер / к-сть можливих пар
        let mset: HashSet<u32> = mem.iter().copied().collect();
        let (mut sstr, mut se) = (0.0f64, 0u32);
        let mut toks: HashMap<String, u32> = HashMap::new();
        for &(a, b, nrm, _) in &edges {
            if mset.contains(&a) && mset.contains(&b) {
                sstr += nrm;
                se += 1;
                if let Some(tk) = pair_tok.get(&(a, b)) {
                    for (t, c) in tk {
                        *toks.entry(t.clone()).or_insert(0) += c;
                    }
                }
            }
        }
        let mut top_toks: Vec<(String, u32)> = toks.into_iter().collect();
        top_toks.sort_by(|x, y| y.1.cmp(&x.1));
        let top_syms: Vec<String> = top_toks
            .iter()
            .take(6)
            .map(|(tk, c)| format!("{}×{}", symbol.get(tk).filter(|s| !s.is_empty()).cloned().unwrap_or_else(|| tk.split('\t').nth(1).unwrap_or("?")[..8.min(tk.split('\t').nth(1).unwrap_or("?").len())].to_string()), c))
            .collect();
        let addrs: Vec<String> = mem.iter().map(|&w| waddr[w as usize].clone()).collect();
        groups.push(json!({
            "size": mem.len(),
            "edges": se,
            "strength": (sstr * 1000.0).round() / 1000.0,
            "avgEdge": ((sstr / se.max(1) as f64) * 1000.0).round() / 1000.0,
            "nTokens": top_toks.len(),
            "topSharedTokens": top_syms,
            "wallets": addrs,
        }));
    }
    // сортуємо за силою × розмір
    groups.sort_by(|a, b| {
        let ka = a["strength"].as_f64().unwrap_or(0.0);
        let kb = b["strength"].as_f64().unwrap_or(0.0);
        kb.partial_cmp(&ka).unwrap()
    });
    let total = groups.len();
    groups.truncate(top);

    // ── Кластеризація ГРУП у архетипи (k-means на [ln size, ln strength, avgEdge, ln nTokens]) ──
    let gk = 4usize.min(groups.len());
    let mut arche_summary: Vec<Value> = Vec::new();
    if gk >= 2 {
        let gf: Vec<[f64; 4]> = groups.iter().map(|g| [
            g["size"].as_f64().unwrap_or(1.0).max(1.0).ln(),
            (1.0 + g["strength"].as_f64().unwrap_or(0.0)).ln(),
            g["avgEdge"].as_f64().unwrap_or(0.0),
            (1.0 + g["nTokens"].as_f64().unwrap_or(1.0)).ln(),
        ]).collect();
        let n = gf.len();
        let mut mean = [0.0f64; 4];
        let mut sd = [0.0f64; 4];
        for x in &gf { for j in 0..4 { mean[j] += x[j]; } }
        for j in 0..4 { mean[j] /= n as f64; }
        for x in &gf { for j in 0..4 { sd[j] += (x[j] - mean[j]).powi(2); } }
        for j in 0..4 { sd[j] = (sd[j] / n as f64).sqrt().max(1e-9); }
        let z: Vec<[f64; 4]> = gf.iter().map(|x| { let mut o = [0.0; 4]; for j in 0..4 { o[j] = (x[j] - mean[j]) / sd[j]; } o }).collect();
        let mut cent: Vec<[f64; 4]> = (0..gk).map(|i| z[i * n / gk]).collect();
        let mut asg = vec![0usize; n];
        for _ in 0..40 {
            for i in 0..n {
                let (mut bi, mut bd) = (0usize, f64::MAX);
                for (c, ct) in cent.iter().enumerate() {
                    let mut d = 0.0; for j in 0..4 { d += (z[i][j] - ct[j]).powi(2); }
                    if d < bd { bd = d; bi = c; }
                }
                asg[i] = bi;
            }
            let mut sum = vec![[0.0f64; 4]; gk];
            let mut cnt = vec![0u32; gk];
            for i in 0..n { cnt[asg[i]] += 1; for j in 0..4 { sum[asg[i]][j] += z[i][j]; } }
            for c in 0..gk { if cnt[c] > 0 { for j in 0..4 { cent[c][j] = sum[c][j] / cnt[c] as f64; } } }
        }
        for i in 0..n { groups[i].as_object_mut().unwrap().insert("archetype".to_string(), json!(asg[i])); }
        for c in 0..gk {
            let mem: Vec<usize> = (0..n).filter(|&i| asg[i] == c).collect();
            if mem.is_empty() { continue; }
            let asz: f64 = mem.iter().map(|&i| groups[i]["size"].as_f64().unwrap_or(0.0)).sum::<f64>() / mem.len() as f64;
            let atk: f64 = mem.iter().map(|&i| groups[i]["nTokens"].as_f64().unwrap_or(0.0)).sum::<f64>() / mem.len() as f64;
            let aed: f64 = mem.iter().map(|&i| groups[i]["avgEdge"].as_f64().unwrap_or(0.0)).sum::<f64>() / mem.len() as f64;
            let label = if asz >= 10.0 { "велика когорта" }
                else if aed >= 1.5 { "тісний сильний картель" }
                else if atk >= 6.0 { "мульти-токен деск" }
                else { "мала група / пара" };
            arche_summary.push(json!({"archetype": c, "groups": mem.len(), "avgSize": (asz*10.0).round()/10.0, "avgTokens": (atk*10.0).round()/10.0, "avgEdge": (aed*100.0).round()/100.0, "label": label}));
        }
        arche_summary.sort_by(|a, b| b["groups"].as_u64().unwrap_or(0).cmp(&a["groups"].as_u64().unwrap_or(0)));
    }

    let out = json!({
        "generated": now(),
        "method": "поведінкова ко-активність (той самий token/day/dir, без прямих лінків); IDF-зважено, нормалізовано по активності; граф→компоненти",
        "params": {"POP": pop, "MAX_GROUP": max_group, "MIN_SHARED": min_shared, "EDGE_MIN": edge_min},
        "totalGroups": total,
        "shown": groups.len(),
        "archetypes": arche_summary.clone(),
        "groups": groups,
    });
    let path = format!("{root}/dataset/wallet-groups.json");
    fs::write(&path, serde_json::to_string_pretty(&out).unwrap()).expect("write");
    eprintln!("[coord] знайдено груп {total} -> {path}");
    println!("=== КООРДИНОВАНІ ГРУПИ (топ за силою) ===");
    for (i, g) in out["groups"].as_array().unwrap().iter().take(25).enumerate() {
        println!(
            "#{:<2} size={:<3} edges={:<4} strength={:<8} avg={:<6} | токени: {}",
            i + 1,
            g["size"].as_u64().unwrap(),
            g["edges"].as_u64().unwrap(),
            g["strength"],
            g["avgEdge"],
            g["topSharedTokens"].as_array().unwrap().iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(", ")
        );
    }
}
