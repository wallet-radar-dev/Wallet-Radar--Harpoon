//! grouppart — ПОВНОЦІННЕ РОЗБИТТЯ гаманців на групи: кожен гаманець належить РІВНО ОДНІЙ групі.
//!
//! Було: групи перекривались (NET∩A), а членство зберігалось обрізаним (2000 з 16868) — через це
//! і капітал рахувався по обрізку. Тепер:
//!   1) граф ТІЛЬКИ EOA↔EOA (ребро = переказ priced-альта або ETH), хаби зі ступенем >HUB_DEG геть;
//!   2) сіди = ядра наявних груп (NET — оригінатори rglead, A/B — члени з role=core);
//!   3) КОНКУРЕНТНИЙ multi-source BFS: гаманець іде до НАЙБЛИЖЧОГО сіда → перекриттів немає;
//!      детермінований тай-брейк: спершу менші (специфічніші) набори сідів, далі за адресою;
//!   4) компоненти без сідів = самостійні групи (успадковують id старих HG, якщо збігаються),
//!      дрібніші за MIN_GROUP — у групу OTHER; біржі/контракти — у INFRA (не капітал).
//! Тобто ЖОДЕН гаманець не лишається без групи, і жоден не в двох.
//!
//! Вихід: dataset/backfill/group-membership.json {wallet:{group,hop}} — повне, авторитетне членство
//!        dataset/backfill/group-partition.json — статистика розбиття
//!        ОНОВЛЮЄ dataset/capital-groups.json (size=повний, members=зріз, membersFile=посилання)
//! Args: ROOT. Env: MIN_GROUP(20) HUB_DEG(300) MEMBERS_CAP(1000) WRITE(1)
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader};

fn is_stable(sym: &str) -> bool {
    let s = sym.to_uppercase();
    s.contains("USD") || s.contains("EUR") || s.contains("GBP")
        || matches!(s.as_str(), "DAI"|"FRAX"|"LUSD"|"TUSD"|"USTB"|"AUSD"|"GHO"|"MIM"|"WBTC"|"CBBTC"|"TBTC"|"WETH"|"WBETH"|"CBETH"|"WSTETH"|"STETH"|"RETH"|"WEETH"|"EETH") || s.ends_with("ON")
}
fn find(p: &mut Vec<u32>, x: u32) -> u32 {
    let mut r = x; while p[r as usize] != r { r = p[r as usize]; }
    let mut c = x; while p[c as usize] != c { let n = p[c as usize]; p[c as usize] = r; c = n; }
    r
}

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let min_group: usize = std::env::var("MIN_GROUP").ok().and_then(|s| s.parse().ok()).unwrap_or(20);
    let hub_deg: usize = std::env::var("HUB_DEG").ok().and_then(|s| s.parse().ok()).unwrap_or(300);
    let cap: usize = std::env::var("MEMBERS_CAP").ok().and_then(|s| s.parse().ok()).unwrap_or(1000);
    let dust_eth: f64 = std::env::var("DUST_ETH").ok().and_then(|s| s.parse().ok()).unwrap_or(0.001);
    let write = std::env::var("WRITE").map(|v| v != "0").unwrap_or(true);
    let bf = format!("{root}/dataset/backfill");

    // --- довідники / фільтри ---
    let mut supply: HashMap<String, String> = HashMap::new();
    if let Ok(sv) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(File::open(format!("{root}/dataset/token-supply.json")).unwrap())) {
        for (c, v) in &sv { supply.insert(c.to_lowercase(), v["symbol"].as_str().unwrap_or("").to_string()); }
    }
    let mut real: HashSet<String> = HashSet::new();
    if let Ok(v) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(File::open(format!("{bf}/onchain-prices.json")).unwrap())) { for k in v.keys() { real.insert(k.to_lowercase()); } }
    // ВАЖЛИВО: НЕ вимагаємо наявності токена в token-supply.json — там лише 167 із 574 цінованих,
    // і стара умова `real ∧ supply` звужувала і граф, і портфелі в ~3.4 раза (167 токенів,
    // 1712 гаманців). Для групування достатньо, що токен має ончейн-ціну і не є стейблом;
    // supply потрібен лише для арифметики емісії.
    let priced_alt = |t: &str, sym: &str| real.contains(t) && !is_stable(sym);
    let mut kind: HashMap<String, String> = HashMap::new();
    if let Ok(v) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(File::open(format!("{bf}/wallet-kind.json")).unwrap())) {
        for (a, k) in &v { kind.insert(a.to_lowercase(), k.as_str().unwrap_or("").to_string()); }
    }
    let lbl: serde_json::Map<String, Value> = serde_json::from_reader(BufReader::new(File::open(format!("{root}/dataset/labels-cache.json")).unwrap())).unwrap_or_default();
    // контракти, доведені кодом на ланцюжку (ethbal фаза 2) — мітки їх пропускають
    let coded: HashSet<String> = serde_json::from_reader::<_, Value>(BufReader::new(File::open(format!("{bf}/eth-balances.json")).unwrap()))
        .ok().and_then(|v| v["wallets"].as_object().map(|m| m.iter().filter(|(_, x)| x["code"] == json!(true)).map(|(k, _)| k.to_lowercase()).collect())).unwrap_or_default();
    let is_cex = |a: &str| matches!(kind.get(a).map(|s| s.as_str()), Some("exchange") | Some("otc"))
        || lbl.get(a).and_then(|x| x["name"].as_str()).map(|n| { let l = n.to_lowercase();
            ["binance","coinbase","okx","gate","kraken","bybit","kucoin","bitget","huobi","mexc","bitfinex","upbit","bithumb","uphold","paribu","btcturk","bingx","gopax","crypto.com","deposit funder","hot wallet","wintermute","galaxy","falconx"].iter().any(|c| l.contains(c)) }).unwrap_or(false);
    let is_infra = |a: &str| is_cex(a) || kind.get(a).map(|s| s == "contract").unwrap_or(false)
        || coded.contains(a) || supply.contains_key(a) || real.contains(a);
    let bad = |a: &str| a.is_empty() || a.len() != 42 || a.starts_with("0x000000000000000000000000000000000000") || a.contains("dead") || is_infra(a);

    // --- граф EOA↔EOA ---
    let mut id: HashMap<String, u32> = HashMap::new();
    let mut names: Vec<String> = Vec::new();
    let getid = |s: &str, id: &mut HashMap<String, u32>, names: &mut Vec<String>| -> u32 {
        if let Some(&i) = id.get(s) { i } else { let i = names.len() as u32; id.insert(s.to_string(), i); names.push(s.to_string()); i }
    };
    let mut edges: Vec<(u32, u32)> = Vec::new();
    let mut seen: HashSet<(u32, u32)> = HashSet::new();
    let mut val_edge: HashSet<(u32, u32)> = HashSet::new();          // ребра з відомою (не-дрібʼязковою) сумою
    let mut port: HashMap<String, HashSet<u32>> = HashMap::new();   // гаманець -> набір priced-альтів
    let mut tok_id: HashMap<String, u32> = HashMap::new();
    let mut tok_names: Vec<String> = Vec::new();
    for line in BufReader::with_capacity(1 << 20, File::open(format!("{bf}/transfers.csv")).unwrap()).lines().map_while(Result::ok) {
        let mut it = line.splitn(6, ',');
        let w = it.next().unwrap_or(""); let _c = it.next(); let d = it.next().unwrap_or(""); let _t = it.next();
        let tok = it.next().unwrap_or(""); let rest = it.next().unwrap_or("");
        if w == "wallet" || (d != "i" && d != "o") { continue; }
        let sym0 = rest.split(',').next().unwrap_or("");
        let wl = w.to_lowercase(); if bad(&wl) { continue; }
        // ПОРТФЕЛЬ для фази 2 — БУДЬ-ЯКИЙ нестейбл-токен, не лише цінований: рідкісний спільний
        // токен (хай навіть обскурний) — це сильніший сигнал належності, ніж популярний; вагу
        // дає IDF, тож спам-ейрдропи, розкидані на тисячі адрес, самі знецінюються.
        if sym0 != "ETH" && !is_stable(sym0) {
            let ti = *tok_id.entry(tok.to_lowercase()).or_insert_with(|| { tok_names.push(tok.to_lowercase()); (tok_names.len() - 1) as u32 });
            port.entry(wl.clone()).or_default().insert(ti);
        }
        if sym0 != "ETH" && !priced_alt(&tok.to_lowercase(), sym0) { continue; }
        // ДРІБʼЯЗОК НЕ Є ЗВʼЯЗКОМ: 1e-9 ETH — це спам адресного отруєння, а не стосунки гаманців
        let amt: f64 = line.rsplitn(4, ',').nth(1).unwrap_or("0").parse().unwrap_or(0.0);
        if sym0 == "ETH" && amt < dust_eth { continue; }
        let cp = line.rsplitn(4, ',').nth(2).unwrap_or("").to_lowercase(); if bad(&cp) || cp == wl { continue; }
        let ia = getid(&wl, &mut id, &mut names); let ib = getid(&cp, &mut id, &mut names);
        let e = if ia < ib { (ia, ib) } else { (ib, ia) };
        if seen.insert(e) { edges.push(e); }
        val_edge.insert(e);
    }
    // нативні ETH-ребра З СУМАМИ (eth_edges2) — повноцінні звʼязки: суму видно, спам відсіяно
    if let Ok(f) = File::open(format!("{bf}/eth-edges2.jsonl")) {
        for line in BufReader::new(f).lines().map_while(Result::ok) {
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            let w = v["w"].as_str().unwrap_or("").to_lowercase(); if bad(&w) { continue; }
            for c in v["cps"].as_array().cloned().unwrap_or_default() {
                let cp = c["a"].as_str().unwrap_or("").to_lowercase(); if bad(&cp) || cp == w { continue; }
                let eth = c["i"].as_f64().unwrap_or(0.0) + c["o"].as_f64().unwrap_or(0.0);
                if eth < dust_eth { continue; }
                let ia = getid(&w, &mut id, &mut names); let ib = getid(&cp, &mut id, &mut names);
                let e = if ia < ib { (ia, ib) } else { (ib, ia) };
                if seen.insert(e) { edges.push(e); }
                val_edge.insert(e);
            }
        }
    }
    // старий докраул без сум — лишається як слабкий сигнал (не valued)
    if let Ok(f) = File::open(format!("{bf}/eth-edges.jsonl")) {
        for line in BufReader::new(f).lines().map_while(Result::ok) {
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            let w = v["w"].as_str().unwrap_or("").to_lowercase(); if bad(&w) { continue; }
            for c in v["cps"].as_array().cloned().unwrap_or_default() {
                let cp = c.as_str().unwrap_or("").to_lowercase(); if bad(&cp) || cp == w { continue; }
                let ia = getid(&w, &mut id, &mut names); let ib = getid(&cp, &mut id, &mut names);
                let e = if ia < ib { (ia, ib) } else { (ib, ia) };
                if seen.insert(e) { edges.push(e); }
            }
        }
    }
    // всі відстежувані гаманці теж мають бути у всесвіті (навіть без ребер)
    if let Ok(f) = File::open(format!("{bf}/backfill-wallets.txt")) {
        for l in BufReader::new(f).lines().map_while(Result::ok) {
            let a = l.trim().to_lowercase();
            if a.len() == 42 && a.starts_with("0x") && !bad(&a) { getid(&a, &mut id, &mut names); }
        }
    }
    let n = names.len();
    eprintln!("[grouppart] всесвіт EOA: {n} · ребер: {}", edges.len());

    // хаби (роутери/міксери) не протягуємо
    let mut deg = vec![0usize; n];
    for (a, b) in &edges { deg[*a as usize] += 1; deg[*b as usize] += 1; }
    let hub: Vec<bool> = deg.iter().map(|d| *d > hub_deg).collect();
    let hubs = hub.iter().filter(|h| **h).count();
    let mut adj: Vec<Vec<u32>> = vec![Vec::new(); n];
    let mut adjv: Vec<Vec<u32>> = vec![Vec::new(); n];   // лише ребра з ПЕРЕВІРЕНОЮ сумою (transfers)
    let mut kept = 0usize;
    for (a, b) in &edges {
        if hub[*a as usize] || hub[*b as usize] { continue; }
        adj[*a as usize].push(*b); adj[*b as usize].push(*a); kept += 1;
        if val_edge.contains(&(*a, *b)) { adjv[*a as usize].push(*b); adjv[*b as usize].push(*a); }
    }
    for v in adj.iter_mut() { v.sort_unstable(); }
    for v in adjv.iter_mut() { v.sort_unstable(); }
    eprintln!("[grouppart] ребер із перевіреною сумою: {} з {kept}", val_edge.len());
    eprintln!("[grouppart] хабів (ступінь >{hub_deg}) виключено: {hubs} · ребер лишилось: {kept}");

    // --- компоненти ---
    let mut par: Vec<u32> = (0..n as u32).collect();
    for (a, b) in &edges {
        if hub[*a as usize] || hub[*b as usize] { continue; }
        let (ra, rb) = (find(&mut par, *a), find(&mut par, *b));
        if ra != rb { par[ra as usize] = rb; }
    }
    let mut comp: Vec<u32> = vec![0; n];
    for i in 0..n as u32 { comp[i as usize] = find(&mut par, i); }
    let mut comp_size: HashMap<u32, usize> = HashMap::new();
    for c in &comp { *comp_size.entry(*c).or_insert(0) += 1; }

    // --- сіди груп ---
    let cgpath = format!("{root}/dataset/capital-groups.json");
    let mut cg: Value = serde_json::from_reader(BufReader::new(File::open(&cgpath).expect("capital-groups"))).expect("cg parse");
    let old_groups = cg["groups"].as_array().cloned().unwrap_or_default();
    let mut seeds: Vec<(String, Vec<u32>)> = Vec::new();          // (group id, сіди)
    let mut old_members: HashMap<String, HashSet<String>> = HashMap::new();
    // СІДИ — з НЕЗАЛЕЖНИХ джерел, не з capital-groups.json, інакше бін не ідемпотентний:
    // після першого запису службові кошики (NODATA/OTHER/INFRA) самі стали б «групами» з сотнями
    // сідів і поглинули б NET (одного разу так і сталось: «злито NODATA ← NET»).
    //   NET       — оригінатори rglead
    //   HG2, HG3… — компоненти hops-groups.json (окрім HG1: це та сама мережа, що й NET)
    let mut net_cores: Vec<u32> = Vec::new();
    if let Ok(v) = serde_json::from_reader::<_, Value>(BufReader::new(File::open(format!("{bf}/rglead.json")).unwrap())) {
        for w in v["wallets"].as_array().cloned().unwrap_or_default() {
            if w["tier"] == "оригінатор" {
                let a = w["w"].as_str().unwrap_or("").to_lowercase();
                if !bad(&a) { if let Some(&i) = id.get(&a) { net_cores.push(i); } }
            }
        }
    }
    net_cores.sort_unstable(); net_cores.dedup();
    if !net_cores.is_empty() { seeds.push(("NET".into(), net_cores)); }
    if let Ok(v) = serde_json::from_reader::<_, Value>(BufReader::new(File::open(format!("{bf}/hops-groups.json")).unwrap())) {
        for c in v["components"].as_array().cloned().unwrap_or_default() {
            let cid = c["id"].as_str().unwrap_or("").to_string();
            if cid == "HG1" || cid.is_empty() { continue; }
            let mut ms: HashSet<String> = HashSet::new();
            let mut cores: Vec<u32> = Vec::new();
            for m in c["members"].as_array().cloned().unwrap_or_default() {
                let a = m["address"].as_str().or_else(|| m.as_str()).unwrap_or("").to_lowercase();
                if a.len() != 42 { continue; }
                ms.insert(a.clone());
                if !bad(&a) { if let Some(&i) = id.get(&a) { cores.push(i); } }
            }
            old_members.insert(cid.clone(), ms);
            cores.sort_unstable(); cores.dedup();
            if cores.len() >= min_group { seeds.push((cid, cores)); }
        }
    }
    let _ = &old_groups;
    eprintln!("[grouppart] сіди: {}", seeds.iter().map(|(g, s)| format!("{g}={}", s.len())).collect::<Vec<_>>().join(" · "));

    // ЗЛИТТЯ ГРУП, ЯКІ СИДЯТЬ В ОДНІЙ КОМПОНЕНТІ. Дві «різні» групи всередині однієї звʼязної
    // компоненти — це довільний розріз однієї мережі: BFS ділив би її по відстані від сідів
    // (A забирала 14480, NET 1471). Звʼязність — і є визначення групи, тож компонента = група.
    // Ім'я лишає група з більшою кількістю сідів.
    let mut by_comp: HashMap<u32, Vec<usize>> = HashMap::new();
    for (gi, (_, ss)) in seeds.iter().enumerate() {
        // компонента, де більшість сідів групи
        let mut cnt: HashMap<u32, usize> = HashMap::new();
        for &s in ss { *cnt.entry(comp[s as usize]).or_insert(0) += 1; }
        let c = cnt.iter().max_by_key(|(c, n)| (**n, std::cmp::Reverse(**c))).map(|(c, _)| *c).unwrap();
        by_comp.entry(c).or_default().push(gi);
    }
    let mut merged_note: Vec<String> = Vec::new();
    let mut seeds2: Vec<(String, Vec<u32>)> = Vec::new();
    for (_c, gis) in by_comp.iter() {
        // ім'я НЕ має плавати між прогонами: NET — канонічна назва мережі, вона й лишається
        // (без цього при кожному злитті компонент перемагала група з більшою к-стю сідів:
        //  NET → HG5 → HG2, і звіти переставали зіставлятись)
        let mut best = gis[0];
        for gi in gis { if seeds[*gi].1.len() > seeds[best].1.len() { best = *gi; } }
        if let Some(net) = gis.iter().find(|gi| seeds[**gi].0 == "NET") { best = *net; }
        let mut all: Vec<u32> = Vec::new();
        for gi in gis { all.extend(seeds[*gi].1.iter().copied()); }
        all.sort_unstable(); all.dedup();
        if gis.len() > 1 {
            merged_note.push(format!("{} ← {}", seeds[best].0, gis.iter().filter(|g| **g != best).map(|g| seeds[*g].0.clone()).collect::<Vec<_>>().join("+")));
        }
        seeds2.push((seeds[best].0.clone(), all));
    }
    let seeds = { let mut s = seeds2; s.sort_by(|a, b| a.1.len().cmp(&b.1.len()).then(a.0.cmp(&b.0))); s };
    if !merged_note.is_empty() { eprintln!("[grouppart] злито (одна компонента = одна група): {}", merged_note.join(" · ")); }

    // --- ПОШАРОВЕ ПРИЄДНАННЯ: ≥MIN_LINKS незалежних звʼязків із групою ---
    // Одного ребра НЕ досить: адресне отруєння (dust-спам по 1e-9 ETH з адрес-двійників) інакше
    // втягує в групу випадкових китів — так у NET потрапив EOA з 1.03М ETH ($1.97 млрд) на hop3,
    // єдиним «звʼязком» якого був вхідний спам. Правило ≥2 зв'язків це відсікає.
    let min_links: usize = std::env::var("MIN_LINKS").ok().and_then(|s| s.parse().ok()).unwrap_or(2);
    let mut owner: Vec<i32> = vec![-1; n];
    let mut hop: Vec<u32> = vec![u32::MAX; n];
    for (gi, (_, ss)) in seeds.iter().enumerate() {
        for &s in ss { if owner[s as usize] < 0 { owner[s as usize] = gi as i32; hop[s as usize] = 0; } }
    }
    let mut round = 0u32;
    loop {
        round += 1;
        let mut add: Vec<(u32, i32, u32)> = Vec::new();
        for v in 0..n as u32 {
            if owner[v as usize] >= 0 { continue; }
            let mut cnt: HashMap<i32, (usize, u32)> = HashMap::new();   // група -> (звʼязків, мін.хоп)
            for &u in &adj[v as usize] {
                let o = owner[u as usize];
                if o < 0 { continue; }
                let e = cnt.entry(o).or_insert((0, u32::MAX));
                e.0 += 1;
                e.1 = e.1.min(hop[u as usize]);
            }
            // хоча б ОДИН звʼязок має бути з перевіреною сумою: ETH-докраул (eth-edges) сум не має,
            // тож на самих лише його ребрах у групу заходив спам-дрібʼязок адресного отруєння
            let mut vok: HashSet<i32> = HashSet::new();
            for &u in &adjv[v as usize] { let o = owner[u as usize]; if o >= 0 { vok.insert(o); } }
            cnt.retain(|g, _| vok.contains(g));
            // детермінований вибір: більше звʼязків → менший хоп → менший індекс групи
            let mut best: Option<(i32, usize, u32)> = None;
            for (g, (c, h)) in &cnt {
                if *c < min_links { continue; }
                let cand = (*g, *c, *h);
                best = match best { None => Some(cand), Some(b) if (cand.1, std::cmp::Reverse(cand.2), std::cmp::Reverse(cand.0)) > (b.1, std::cmp::Reverse(b.2), std::cmp::Reverse(b.0)) => Some(cand), other => other };
            }
            if let Some((g, _, h)) = best { add.push((v, g, h + 1)); }
        }
        if add.is_empty() { break; }
        for (v, g, h) in add { owner[v as usize] = g; hop[v as usize] = h; }
        if round > 30 { break; }
    }
    eprintln!("[grouppart] приєднання: {round} шарів, правило ≥{min_links} зв'язків із групою");

    // --- компоненти без сідів → власні групи ---
    let mut unseeded: HashMap<u32, Vec<u32>> = HashMap::new();
    for i in 0..n as u32 { if owner[i as usize] < 0 { unseeded.entry(comp[i as usize]).or_default().push(i); } }
    let mut extra: Vec<(u32, Vec<u32>)> = unseeded.into_iter().collect();
    extra.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(&b.0)));
    let mut group_of: HashMap<String, (String, u32)> = HashMap::new();   // wallet -> (group, hop)
    for (gi, (gidv, _)) in seeds.iter().enumerate() {
        for i in 0..n as u32 { if owner[i as usize] == gi as i32 { group_of.insert(names[i as usize].clone(), (gidv.clone(), hop[i as usize])); } }
    }
    let mut new_groups: Vec<(String, usize)> = Vec::new();
    let mut k = 0;
    // у компоненті з сідом ті, хто не набрав MIN_LINKS — це СЛАБКА ПЕРИФЕРІЯ цієї групи
    // (один випадковий переказ), а не окрема група: позначаємо <ГРУПА>-WEAK, щоб різниця була видна
    let mut comp_seed: HashMap<u32, String> = HashMap::new();
    for (gidv, ss) in &seeds { for &s in ss { comp_seed.entry(comp[s as usize]).or_insert_with(|| gidv.clone()); } }
    let mut pending: Vec<u32> = Vec::new();
    for (c, mem) in &extra {
        if let Some(g) = comp_seed.get(c) {
            let w = format!("{g}-WEAK");
            for &i in mem { group_of.insert(names[i as usize].clone(), (w.clone(), u32::MAX)); }
            new_groups.push((w, mem.len()));
            continue;
        }
        if mem.len() < min_group {
            pending.extend(mem.iter().copied());
            continue;
        }
        // успадкувати id старої групи, якщо збігається щонайменше наполовину
        let set: HashSet<&String> = mem.iter().map(|i| &names[*i as usize]).collect();
        let mut gidv = String::new();
        for (oid, oms) in &old_members {
            if seeds.iter().any(|(s, _)| s == oid) { continue; }
            let inter = oms.iter().filter(|a| set.contains(a)).count();
            if !oms.is_empty() && inter * 2 >= oms.len() { gidv = oid.clone(); break; }
        }
        if gidv.is_empty() { k += 1; gidv = format!("CG{k}"); }
        for &i in mem { group_of.insert(names[i as usize].clone(), (gidv.clone(), u32::MAX)); }
        new_groups.push((gidv, mem.len()));
    }

    // --- ФАЗА 2: гаманці БЕЗ ребер розкладаємо за ПОРТФЕЛЕМ ---
    // Причина: 19446 із 27281 відстежуваних не мають жодного EOA↔EOA-ребра до груп (бекфіл
    // whale-центричний, прямі перекази між нашими гаманцями рідкість). Графом їх не розкласти,
    // тож другий критерій — спорідненість портфеля з вагою IDF: рідкісний спільний токен важить
    // багато, USDT — майже нічого. Провенанс кожного члена лишається видимим (поле `via`).
    let min_shared: usize = std::env::var("MIN_SHARED").ok().and_then(|s| s.parse().ok()).unwrap_or(2);
    let idf_min: f64 = std::env::var("IDF_MIN").ok().and_then(|s| s.parse().ok()).unwrap_or(3.0);
    let min_score: f64 = std::env::var("MIN_SCORE").ok().and_then(|s| s.parse().ok()).unwrap_or(6.0);
    let nw = port.len().max(1) as f64;
    let mut df: HashMap<u32, usize> = HashMap::new();
    for ts in port.values() { for t in ts { *df.entry(*t).or_insert(0) += 1; } }
    let idf: HashMap<u32, f64> = df.iter().map(|(t, d)| (*t, (nw / (1.0 + *d as f64)).ln())).collect();
    // профілі груп: які токени тримають уже призначені (графом) члени
    let mut gtok: HashMap<String, HashSet<u32>> = HashMap::new();
    let mut gsz: HashMap<String, usize> = HashMap::new();
    for (w, (g, _)) in &group_of {
        *gsz.entry(g.clone()).or_insert(0) += 1;
        if let Some(ts) = port.get(w) { gtok.entry(g.clone()).or_default().extend(ts.iter().copied()); }
    }
    let real_groups: Vec<String> = { let mut v: Vec<String> = gsz.iter().filter(|(_, n)| **n >= min_group).map(|(g, _)| g.clone()).collect(); v.sort(); v };
    let mut by_behav = 0usize;
    let mut behav: HashSet<String> = HashSet::new();
    let (mut p_port, mut p_scored) = (0usize, 0usize);
    let mut score_hist: Vec<f64> = Vec::new();
    eprintln!("[grouppart] фаза 2: груп-кандидатів {} · токенів у профілях {} · гаманців із портфелем {}",
        real_groups.len(), gtok.values().map(|s| s.len()).sum::<usize>(), port.len());
    for &i in &pending {
        let w = &names[i as usize];
        let Some(ts) = port.get(w) else { group_of.insert(w.clone(), ("NODATA".into(), u32::MAX)); continue };
        p_port += 1;
        let (mut best, mut best_score, mut best_shared) = (String::new(), 0.0f64, 0usize);
        for g in &real_groups {
            let Some(gt) = gtok.get(g) else { continue };
            let (mut score, mut shared) = (0.0f64, 0usize);
            for t in ts {
                if !gt.contains(t) { continue; }
                let w_t = idf.get(t).copied().unwrap_or(0.0);
                score += w_t;
                if w_t >= idf_min { shared += 1; }
            }
            if score > best_score { best_score = score; best = g.clone(); best_shared = shared; }
        }
        if !best.is_empty() && best_shared >= min_shared && best_score >= min_score {
            group_of.insert(w.clone(), (best, u32::MAX)); behav.insert(w.clone()); by_behav += 1;
        } else {
            group_of.insert(w.clone(), ("OTHER".into(), u32::MAX));
        }
        if best_score > 0.0 { p_scored += 1; score_hist.push(best_score); }
    }
    score_hist.sort_by(|a, b| b.partial_cmp(a).unwrap());
    let med = score_hist.get(score_hist.len() / 2).copied().unwrap_or(0.0);
    eprintln!("[grouppart] фаза 2 (портфель): без-ребрових {} · із портфелем {p_port} · з ненульовим збігом {p_scored} · медіана балу {:.1} · РОЗКЛАДЕНО {by_behav}",
        pending.len(), med);
    if !score_hist.is_empty() { eprintln!("[grouppart] топ-бали: {}", score_hist.iter().take(8).map(|s| format!("{s:.1}")).collect::<Vec<_>>().join(" ")); }

    // --- ХАБИ: не протягуємо крізь них шляхи, але й не ховаємо ---
    // Після докраулу контрагентів казна мережі сама стала хабом (2623 звʼязки) і провалилась
    // у NODATA. Через хаб не можна будувати шляхи (інакше «звʼязаними» стануть усі, хто його
    // колись торкнувся), але це не привід губити його: виносимо в окрему групу HUB, де видно
    // і сам вузол, і його реальні гроші.
    for i in 0..n as u32 {
        if hub[i as usize] { group_of.insert(names[i as usize].clone(), ("HUB".into(), u32::MAX)); }
    }

    // --- інфраструктура: окрема «група», щоб жоден гаманець не зник ---
    let mut infra_n = 0usize;
    if let Ok(f) = File::open(format!("{bf}/backfill-wallets.txt")) {
        for l in BufReader::new(f).lines().map_while(Result::ok) {
            let a = l.trim().to_lowercase();
            if a.len() == 42 && a.starts_with("0x") && is_infra(&a) { group_of.insert(a, ("INFRA".into(), u32::MAX)); infra_n += 1; }
        }
    }

    // --- статистика ---
    let mut stats: HashMap<String, (usize, HashMap<u32, usize>)> = HashMap::new();
    for (_w, (g, h)) in &group_of {
        let e = stats.entry(g.clone()).or_insert((0, HashMap::new()));
        e.0 += 1;
        if *h != u32::MAX { *e.1.entry(*h).or_insert(0) += 1; }
    }
    let mut sv: Vec<(&String, &(usize, HashMap<u32, usize>))> = stats.iter().collect();
    sv.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));
    eprintln!("[grouppart] ===== РОЗБИТТЯ: кожен гаманець рівно в одній групі =====");
    for (g, (cnt, hops)) in &sv {
        let mut hv: Vec<(&u32, &usize)> = hops.iter().collect(); hv.sort();
        let hs = hv.iter().take(6).map(|(h, c)| format!("hop{h}:{c}")).collect::<Vec<_>>().join(" ");
        eprintln!("   {:<8} {:>7} гаманців   {hs}", g, cnt);
    }
    eprintln!("[grouppart] разом: {} гаманців (з них інфраструктури {infra_n})", group_of.len());

    // --- виходи ---
    let via_of = |w: &String, h: u32| -> &'static str {
        if h != u32::MAX { "граф" } else if behav.contains(w) { "портфель" } else { "компонента" }
    };
    let memb: serde_json::Map<String, Value> = group_of.iter()
        .map(|(w, (g, h))| (w.clone(), json!({"group": g, "hop": if *h == u32::MAX { Value::Null } else { json!(h) }, "via": via_of(w, *h)}))).collect();
    std::fs::write(format!("{bf}/group-membership.json"), serde_json::to_string(&json!({
        "generated": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs(),
        "method": "конкурентний multi-source BFS від ядер груп; компоненти без сідів = власні групи; кожен гаманець рівно в одній",
        "wallets": memb})).unwrap()).ok();
    let mut gstats: Vec<Value> = Vec::new();
    for (g, (c, h)) in &sv {
        let mut hv: Vec<(&u32, &usize)> = h.iter().collect();
        hv.sort();
        let mut m = serde_json::Map::new();
        for (k, v) in hv { m.insert(k.to_string(), json!(v)); }
        gstats.push(json!({"id": g, "size": c, "byHop": m}));
    }
    std::fs::write(format!("{bf}/group-partition.json"), serde_json::to_string_pretty(&json!({
        "universe": n, "edges": kept, "hubsExcluded": hubs, "minGroup": min_group, "groups": gstats})).unwrap()).ok();

    // --- запис у застосунок ---
    if write {
        std::fs::copy(&cgpath, format!("{cgpath}.bak-part")).ok();
        let mut by_group: HashMap<String, Vec<(String, u32)>> = HashMap::new();
        for (w, (g, h)) in &group_of { by_group.entry(g.clone()).or_default().push((w.clone(), *h)); }
        for v in by_group.values_mut() { v.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0))); }

        let mut out: Vec<Value> = Vec::new();
        let mut used: HashSet<String> = HashSet::new();
        for g in &old_groups {                       // зберігаємо наявні групи з їхніми анотаціями
            let gidv = g["id"].as_str().unwrap_or("").to_string();
            let Some(mem) = by_group.get(&gidv) else { continue };
            let mut g2 = g.clone();
            g2["size"] = json!(mem.len());
            g2["members"] = json!(mem.iter().take(cap).map(|(w, h)| json!({"address": w, "hop": if *h == u32::MAX { Value::Null } else { json!(h) }})).collect::<Vec<Value>>());
            g2["membersFile"] = json!("dataset/backfill/group-membership.json");
            g2["partition"] = json!("кожен гаманець рівно в одній групі (конкурентний BFS від ядер)");
            out.push(g2);
            used.insert(gidv);
        }
        for (gidv, mem) in by_group.iter() {         // нові групи з безсідних компонент + OTHER/INFRA
            if used.contains(gidv) { continue; }
            let name = match gidv.as_str() {
                "OTHER" => "Поза мережею — дрібні ізольовані гаманці".to_string(),
                "INFRA" => "Інфраструктура (біржі, контракти, пули) — НЕ капітал групи".to_string(),
                _ => format!("Окрема компонента — {} гаманців", mem.len()),
            };
            out.push(json!({"id": gidv, "name": name, "size": mem.len(),
                "members": mem.iter().take(cap).map(|(w, _)| json!({"address": w})).collect::<Vec<Value>>(),
                "membersFile": "dataset/backfill/group-membership.json",
                "partition": "кожен гаманець рівно в одній групі (конкурентний BFS від ядер)"}));
        }
        out.sort_by(|a, b| b["size"].as_u64().unwrap_or(0).cmp(&a["size"].as_u64().unwrap_or(0)));
        cg["groups"] = json!(out);
        cg["partitioned"] = json!(true);
        std::fs::write(&cgpath, serde_json::to_string_pretty(&cg).unwrap()).ok();
        eprintln!("[grouppart] capital-groups.json оновлено: {} груп (бекап .bak-part)", cg["groups"].as_array().map(|a| a.len()).unwrap_or(0));
    }
    eprintln!("[grouppart] збережено group-membership.json / group-partition.json");
}
