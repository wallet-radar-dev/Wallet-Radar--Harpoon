//! regroup — перебудова членства від ДОВЕДЕНОГО ЯДРА, а не від широкого списку.
//!
//! Навіщо: у списку 6864 вже знайдено Balancer Vault, genesis-адресу Uniswap і CAMP-воронку,
//! у якої 93% контрагентів поза групою. Кожна така адреса тягне мільйони й зсуває підсумки —
//! це сталося чотири рази за одну сесію. Ред-тім раніше показав: при порозі «хаб = вузол
//! з >300 зв'язків» мережа 6283, при порозі >40 — лише 139. Тобто 98% членства тримається
//! на зв'язках через вузли середнього ступеня.
//!
//! Правила побудови (усі три обов'язкові):
//!   1. ГРАФ ЛИШЕ EOA↔EOA — контракти виключено (`contract-flags.json`). Пул чи роутер
//!      зв'язує всіх з усіма, і через нього мережа «злипається» штучно.
//!   2. ХАБИ ГЕТЬ — вузол зі ступенем > HUB_MAX не проводить зв'язок далі. Біржа,
//!      сервіс чи роутер інакше з'єднає тисячі непов'язаних адрес.
//!   3. ПРИЄДНАННЯ ЗА ДОКАЗОМ — адреса входить у групу, лише якщо має ≥ MIN_LINKS
//!      незалежних зв'язків із уже підтвердженими членами, і ≥1 із них має суму ≥ MIN_ETH.
//!      Один випадковий переказ членства не дає.
//!
//! Ядро (сіди) визначається так само, але з жорсткішим порогом хаба (CORE_HUB), тож у нього
//! потрапляють лише вузли, чиї зв'язки не йдуть через сервіси.
//!
//! Вихід: dataset/backfill/regroup-members.txt + regroup.json (з хопами й доказами).
//! Args: ROOT [SEED_FILE]. Env: HUB_MAX(40) CORE_HUB(12) MIN_LINKS(2) MIN_ETH(0.01) MAX_HOP(4)
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet, VecDeque};
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

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let bf = format!("{root}/dataset/backfill");
    let hub_max: usize = envn("HUB_MAX", 40);
    let core_hub: usize = envn("CORE_HUB", 12);
    let min_links: usize = envn("MIN_LINKS", 2);
    let min_eth: f64 = envn("MIN_ETH", 0.01);
    let max_hop: u32 = envn("MAX_HOP", 4);

    let members: HashSet<String> = BufReader::new(File::open(format!("{bf}/net-members.txt")).unwrap())
        .lines().map_while(Result::ok).map(|l| l.trim().to_lowercase())
        .filter(|l| l.len() == 42).collect();

    let mut is_contract: HashSet<String> = HashSet::new();
    if let Ok(f) = File::open(format!("{bf}/contract-flags.json")) {
        if let Ok(m) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(f)) {
            for (a, v) in m { if v.as_bool() == Some(true) { is_contract.insert(a.to_lowercase()); } }
        }
    }
    eprintln!("[regroup] вихідний список {} · відомих контрактів {}", members.len(), is_contract.len());

    // --- прохід: ребра EOA↔EOA із сумами ---
    // Тримаємо тільки ребра, де ОБИДВА кінці у вихідному списку: питання не «хто ще існує»,
    // а «хто з наявного списку справді пов'язаний».
    let mut edge_sum: HashMap<(u32, u32), f64> = HashMap::new();
    let mut idx: HashMap<String, u32> = HashMap::new();
    let mut names: Vec<String> = Vec::new();
    let mut deg_raw: HashMap<u32, HashSet<u32>> = HashMap::new();

    let rdr = BufReader::with_capacity(1 << 22,
        File::open(format!("{bf}/txfull-1.jsonl")).expect("txfull"));
    let mut seen: HashSet<u64> = HashSet::new();
    let mut cur_w = String::new();
    let (mut n, mut used) = (0u64, 0u64);

    for line in rdr.lines().map_while(Result::ok) {
        n += 1;
        if n % 20_000_000 == 0 { eprintln!("    ...{n} рядків, ребер {}", edge_sum.len()); }
        let Some(w) = field(&line, "w") else { continue };
        if !members.contains(w) { continue }
        if w != cur_w { cur_w = w.to_string(); seen.clear(); }
        let Ok(t) = serde_json::from_str::<Value>(&line) else { continue };
        let kind = t["k"].as_str().unwrap_or("");
        if kind != "ext" && kind != "int" { continue }      // лише нативний ETH: у нього видно суму

        let frm = t["f"].as_str().unwrap_or("").to_lowercase();
        let to = t["t"].as_str().unwrap_or("").to_lowercase();
        if frm.is_empty() || to.is_empty() || frm == to { continue }
        if !members.contains(frm.as_str()) || !members.contains(to.as_str()) { continue }
        if is_contract.contains(&frm) || is_contract.contains(&to) { continue }

        let mut h: u64 = 1469598103934665603;
        for s in [t["h"].as_str().unwrap_or(""), frm.as_str(), to.as_str(), t["v"].as_str().unwrap_or("")] {
            for b in s.as_bytes() { h ^= *b as u64; h = h.wrapping_mul(1099511628211) }
        }
        if !seen.insert(h) { continue }

        let v: f64 = t["v"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0.0) / 1e18;
        if v < min_eth { continue }                          // пил і отруєння — не зв'язок
        used += 1;

        let id = |s: &str, idx: &mut HashMap<String, u32>, names: &mut Vec<String>| -> u32 {
            if let Some(i) = idx.get(s) { *i } else {
                let i = names.len() as u32;
                names.push(s.to_string());
                idx.insert(s.to_string(), i);
                i
            }
        };
        let a = id(&frm, &mut idx, &mut names);
        let b = id(&to, &mut idx, &mut names);
        let key = if a < b { (a, b) } else { (b, a) };
        *edge_sum.entry(key).or_insert(0.0) += v;
        deg_raw.entry(a).or_default().insert(b);
        deg_raw.entry(b).or_default().insert(a);
    }
    eprintln!("[regroup] рядків {n} · валідних переказів {used} · вузлів {} · ребер {}",
        names.len(), edge_sum.len());

    // --- список суміжності без хабів ---
    let build_adj = |limit: usize| -> HashMap<u32, Vec<u32>> {
        let mut adj: HashMap<u32, Vec<u32>> = HashMap::new();
        for (&(a, b), _) in &edge_sum {
            let da = deg_raw.get(&a).map(|s| s.len()).unwrap_or(0);
            let db = deg_raw.get(&b).map(|s| s.len()).unwrap_or(0);
            if da > limit || db > limit { continue }        // через хаб зв'язок не проводимо
            adj.entry(a).or_default().push(b);
            adj.entry(b).or_default().push(a);
        }
        adj
    };

    // --- ЯДРО: найбільша компонента при жорсткому порозі ---
    let core_adj = build_adj(core_hub);
    let mut seen_c: HashSet<u32> = HashSet::new();
    let mut best: Vec<u32> = Vec::new();
    for &s in core_adj.keys() {
        if seen_c.contains(&s) { continue }
        let mut comp = Vec::new();
        let mut q = VecDeque::from([s]);
        seen_c.insert(s);
        while let Some(x) = q.pop_front() {
            comp.push(x);
            for &y in core_adj.get(&x).map(|v| v.as_slice()).unwrap_or(&[]) {
                if seen_c.insert(y) { q.push_back(y) }
            }
        }
        if comp.len() > best.len() { best = comp }
    }
    eprintln!("[regroup] ЯДРО (поріг хаба {core_hub}): {} адрес", best.len());

    // --- нарощування: лише за ≥MIN_LINKS незалежними зв'язками, ≥1 із сумою ---
    let adj = build_adj(hub_max);
    let mut member: HashMap<u32, u32> = best.iter().map(|&x| (x, 0u32)).collect();  // вузол -> хоп
    let mut frontier: Vec<u32> = best.clone();
    for hop in 1..=max_hop {
        let mut cand: HashMap<u32, (usize, f64)> = HashMap::new();
        for &m in &frontier {
            for &y in adj.get(&m).map(|v| v.as_slice()).unwrap_or(&[]) {
                if member.contains_key(&y) { continue }
                let key = if m < y { (m, y) } else { (y, m) };
                let s = edge_sum.get(&key).copied().unwrap_or(0.0);
                let e = cand.entry(y).or_insert((0, 0.0));
                e.0 += 1;
                if s > e.1 { e.1 = s }
            }
        }
        let mut added = Vec::new();
        for (y, (links, maxsum)) in cand {
            if links >= min_links && maxsum >= min_eth {
                member.insert(y, hop);
                added.push(y);
            }
        }
        eprintln!("[regroup] хоп {hop}: приєднано {} · разом {}", added.len(), member.len());
        if added.is_empty() { break }
        frontier = added;
    }

    // --- вихід ---
    let mut by_hop: HashMap<u32, u32> = HashMap::new();
    let mut out_addrs: Vec<(String, u32)> = Vec::new();
    for (&i, &hop) in &member {
        *by_hop.entry(hop).or_insert(0) += 1;
        out_addrs.push((names[i as usize].clone(), hop));
    }
    out_addrs.sort();
    let mut f = File::create(format!("{bf}/regroup-members.txt")).unwrap();
    for (a, _) in &out_addrs { let _ = writeln!(f, "{a}"); }

    let dropped: Vec<&String> = members.iter().filter(|m| !idx.contains_key(*m)
        || !member.contains_key(idx.get(*m).unwrap())).collect();

    eprintln!("\n=== ПІДСУМОК ПЕРЕБУДОВИ ===");
    eprintln!("було у списку:        {}", members.len());
    eprintln!("ЯДРО (hop 0):         {}", by_hop.get(&0).copied().unwrap_or(0));
    for h in 1..=max_hop {
        if let Some(c) = by_hop.get(&h) { eprintln!("  + хоп {h}:            {c}") }
    }
    eprintln!("ПІДТВЕРДЖЕНО РАЗОМ:   {}", member.len());
    eprintln!("ВІДСІЯНО:             {} ({:.1}%)", dropped.len(),
        100.0 * dropped.len() as f64 / members.len() as f64);

    let _ = File::create(format!("{bf}/regroup.json")).unwrap().write_all(
        serde_json::to_string(&json!({
            "params": {"HUB_MAX": hub_max, "CORE_HUB": core_hub, "MIN_LINKS": min_links,
                       "MIN_ETH": min_eth, "MAX_HOP": max_hop},
            "wasListed": members.len(), "confirmed": member.len(), "dropped": dropped.len(),
            "byHop": by_hop.iter().map(|(h, c)| (h.to_string(), *c)).collect::<HashMap<_,_>>(),
            "members": out_addrs.iter().map(|(a, h)| json!({"addr": a, "hop": h})).collect::<Vec<_>>(),
        })).unwrap().as_bytes());
    eprintln!("\n[regroup] збережено regroup-members.txt і regroup.json");
}
