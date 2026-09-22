//! Куди 39-деск (ETH-ротації-когорта) спрямовує native-ETH далі. Читає членів деску +
//! когорту з cohort-analysis.json, тягне Etherscan txlist+internal (від члена = вихід),
//! агрегує ETH по `to`-адресі з STARTDATE. Класифікує отримувача: internal (сам деск) /
//! cohort (359) / exchange (nonce>NONCE_CEX через eth_getTransactionCount) / external.
//! Вихід: dataset/backfill/desk-flow.json + звіт. Env: STARTDATE(2025-09-01) TOPN(25) NONCE_CEX(200000).

use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::BufReader;
use std::sync::atomic::{AtomicUsize, Ordering};

fn es_url(keys: &[String], kc: &AtomicUsize, params: &str) -> String {
    let key = &keys[kc.fetch_add(1, Ordering::Relaxed) % keys.len()];
    format!("https://api.etherscan.io/v2/api?chainid=1&{params}&apikey={key}")
}
async fn es(client: &reqwest::Client, keys: &[String], kc: &AtomicUsize, params: &str) -> Value {
    for _ in 0..4 {
        if let Ok(r) = client.get(es_url(keys, kc, params)).send().await {
            if let Ok(v) = r.json::<Value>().await { return v; }
        }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }
    json!({"result": []})
}
async fn startblock(client: &reqwest::Client, keys: &[String], kc: &AtomicUsize, start: &str) -> u64 {
    let ts = chrono::NaiveDate::parse_from_str(start, "%Y-%m-%d").ok()
        .and_then(|d| d.and_hms_opt(0, 0, 0)).map(|dt| dt.and_utc().timestamp()).unwrap_or(0);
    let v = es(client, keys, kc, &format!("module=block&action=getblocknobytime&timestamp={ts}&closest=before")).await;
    v["result"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0)
}

/// Вихідні native-ETH від addr -> (to, wei) з startblock.
async fn outflows(client: &reqwest::Client, keys: &[String], kc: &AtomicUsize, addr: &str, action: &str, sb: u64, acc: &mut HashMap<String, u128>) {
    let w = addr.to_lowercase();
    let mut page_sb = sb;
    for _ in 0..200 {
        let v = es(client, keys, kc, &format!(
            "module=account&action={action}&address={addr}&startblock={page_sb}&endblock=99999999&page=1&offset=1000&sort=asc")).await;
        let Some(res) = v["result"].as_array() else { break };
        if res.is_empty() { break; }
        for t in res {
            if t["isError"].as_str() == Some("1") { continue; }
            let value: u128 = t["value"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0);
            if value == 0 { continue; }
            let from = t["from"].as_str().unwrap_or("").to_lowercase();
            let to = t["to"].as_str().unwrap_or("").to_lowercase();
            if from == w && !to.is_empty() {
                *acc.entry(to).or_insert(0) += value;
            }
        }
        if res.len() < 1000 { break; }
        let nb: u64 = res.last().and_then(|t| t["blockNumber"].as_str()).and_then(|s| s.parse().ok()).unwrap_or(0);
        if nb <= page_sb { break; }
        page_sb = nb;
    }
}

async fn nonce(client: &reqwest::Client, keys: &[String], kc: &AtomicUsize, addr: &str) -> u64 {
    let v = es(client, keys, kc, &format!("module=proxy&action=eth_getTransactionCount&address={addr}&tag=latest")).await;
    v["result"].as_str().and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok()).unwrap_or(0)
}
async fn is_contract(client: &reqwest::Client, keys: &[String], kc: &AtomicUsize, addr: &str) -> bool {
    let v = es(client, keys, kc, &format!("module=proxy&action=eth_getCode&address={addr}&tag=latest")).await;
    v["result"].as_str().map(|s| s.len() > 2 && s != "0x").unwrap_or(false)
}

fn ca_flow(root: &str) -> Vec<(String, f64)> {
    let v: Value = serde_json::from_reader(BufReader::new(
        File::open(format!("{root}/dataset/backfill/desk-flow.json")).expect("desk-flow.json"))).expect("parse");
    v["top"].as_array().unwrap().iter()
        .filter_map(|t| Some((t["to"].as_str()?.to_string(), t["eth"].as_f64()?)))
        .collect()
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let start = std::env::var("STARTDATE").unwrap_or_else(|_| "2025-09-01".into());
    let topn: usize = std::env::var("TOPN").ok().and_then(|s| s.parse().ok()).unwrap_or(25);
    let nonce_cex: u64 = std::env::var("NONCE_CEX").ok().and_then(|s| s.parse().ok()).unwrap_or(200_000);

    let ca: Value = serde_json::from_reader(BufReader::new(
        File::open(format!("{root}/dataset/backfill/cohort-analysis.json")).expect("cohort-analysis.json"))).expect("parse");
    let desk: Vec<String> = ca["communities"][0]["members"].as_array().unwrap()
        .iter().filter_map(|v| v.as_str().map(|s| s.to_lowercase())).collect();
    let cohort: HashSet<String> = ca["band_members"].as_array().unwrap()
        .iter().filter_map(|v| v.as_str().map(|s| s.to_lowercase())).collect();
    let deskset: HashSet<&String> = desk.iter().collect();
    eprintln!("[desk-flow] деск: {} членів, когорта: {}", desk.len(), cohort.len());

    let keys: Vec<String> = fs::read_to_string(format!("{root}/.env")).unwrap_or_default()
        .lines().find(|l| l.starts_with("BACKFILL_ETHERSCAN_KEYS="))
        .map(|l| l.splitn(2, '=').nth(1).unwrap_or("").split(',').map(|k| k.trim().to_string()).filter(|k| !k.is_empty()).collect())
        .unwrap_or_default();
    assert!(!keys.is_empty(), "нема BACKFILL_ETHERSCAN_KEYS");
    let kc = AtomicUsize::new(0);
    let client = reqwest::Client::builder().user_agent("wallet-radar-deskflow/0.1").pool_max_idle_per_host(8).build().unwrap();

    // Швидкий режим: перекласифікувати збережений top (contract/exchange/cohort/external-EOA) без пере-фетчу.
    if std::env::var("CLASSIFY_ONLY").is_ok() {
        let top = ca_flow(&root);
        println!("=== ПЕРЕКЛАСИФІКАЦІЯ top (contract=code, exchange=nonce) ===");
        let mut cls: HashMap<&str, f64> = HashMap::new();
        for (to, eth) in &top {
            let label = if deskset.contains(to) { "internal-desk" }
                else if cohort.contains(to) { "cohort-359" }
                else if is_contract(&client, &keys, &kc, to).await { "contract" }
                else if nonce(&client, &keys, &kc, to).await >= nonce_cex { "exchange/hot" }
                else { "external-EOA" };
            *cls.entry(label).or_insert(0.0) += eth;
            println!("  {eth:>10.0} ETH  {label:<14} {to}");
        }
        println!("\nкласи (по топ):");
        let mut cv: Vec<(&&str, &f64)> = cls.iter().collect();
        cv.sort_by(|a, b| b.1.partial_cmp(a.1).unwrap());
        for (c, v) in cv { println!("  {c:<14} {:.0} ETH", v); }
        return;
    }

    let sb = startblock(&client, &keys, &kc, &start).await;
    eprintln!("startblock ({start}): {sb}");

    // Режим LINK: чи топ-отримувачі шлють ETH НАЗАД у мережу (деск ∪ отримувачі)?
    // Якщо так — це пов'язана мережа; зберігаємо connected-адреси + ребра.
    if std::env::var("MODE").as_deref() == Ok("link") {
        let recips: Vec<(String, f64)> = ca_flow(&root);
        let mut netset: HashSet<String> = deskset.iter().map(|s| (*s).clone()).collect();
        for (a, _) in &recips { netset.insert(a.clone()); }
        eprintln!("[link] мережа: {} деск + {} отримувачів = {} адрес", desk.len(), recips.len(), netset.len());
        let mut connected = Vec::new();
        let mut edges = Vec::new();
        let mut closure_eth = 0.0f64;
        for (r, recv) in &recips {
            if deskset.contains(r) { continue; } // сам деск — вже в мережі
            let mut o: HashMap<String, u128> = HashMap::new();
            outflows(&client, &keys, &kc, r, "txlist", sb, &mut o).await;
            outflows(&client, &keys, &kc, r, "txlistinternal", sb, &mut o).await;
            // ETH від r у мережу
            let mut into_net = 0.0f64;
            let mut tgts: Vec<(String, f64)> = Vec::new();
            for (to, wei) in &o {
                if netset.contains(to) && to != r {
                    let e = *wei as f64 / 1e18;
                    into_net += e;
                    tgts.push((to.clone(), e));
                }
            }
            tgts.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
            if into_net > 1.0 {
                closure_eth += into_net;
                let in_cohort = cohort.contains(r);
                println!("  ✓ {r}  отримав {recv:.0} ← деск | шле {into_net:.0} ETH → мережу ({} ребер){}",
                    tgts.len(), if in_cohort { " [cohort-359]" } else { "" });
                for (t, e) in tgts.iter().take(3) { println!("        → {e:.0} ETH  {t}"); }
                connected.push(json!({"addr": r, "recv_from_desk": recv.round(),
                    "sends_into_net": into_net.round(), "in_cohort": in_cohort,
                    "top_targets": tgts.iter().take(5).map(|(t,e)| json!([t, e.round()])).collect::<Vec<_>>()}));
                for (t, e) in &tgts { edges.push(json!([r, t, e.round()])); }
            } else {
                println!("  ✗ {r}  отримав {recv:.0} ← деск | у мережу шле ~0 (не замкнений)");
            }
        }
        println!("\nзамкнення: {} з {} отримувачів шлють ETH назад у мережу; сумарно {closure_eth:.0} ETH циркулює",
            connected.len(), recips.iter().filter(|(a,_)| !deskset.contains(a)).count());
        serde_json::to_writer_pretty(
            File::create(format!("{root}/dataset/backfill/desk-linked-network.json")).unwrap(),
            &json!({"start": start, "desk": desk, "connected_recipients": connected,
                    "closure_eth": closure_eth.round(), "edges": edges})).unwrap();
        eprintln!("збережено: desk-linked-network.json");
        return;
    }

    // Режим TRACE: розкрутити довільні хаби (SEEDS=addr,addr) ще на крок — куди їхній вихід.
    if std::env::var("MODE").as_deref() == Ok("trace") {
        let seeds: Vec<String> = std::env::var("SEEDS").unwrap_or_default()
            .split(',').map(|s| s.trim().to_lowercase()).filter(|s| s.len() == 42).collect();
        // відома мережа (для мітки "back-to-network")
        let mut known: HashSet<String> = deskset.iter().map(|s| (*s).clone()).collect();
        if let Ok(v) = serde_json::from_reader::<_, Value>(BufReader::new(
            File::open(format!("{root}/dataset/backfill/desk-linked-network.json")).unwrap())) {
            if let Some(cr) = v["connected_recipients"].as_array() {
                for c in cr { if let Some(a) = c["addr"].as_str() { known.insert(a.to_lowercase()); } }
            }
            if let Some(es) = v["edges"].as_array() {
                for e in es { if let Some(a) = e[1].as_str() { known.insert(a.to_lowercase()); } }
            }
        }
        eprintln!("[trace] seeds: {}, відомої мережі: {}", seeds.len(), known.len());
        let mut trace_out = Vec::new();
        for s in &seeds {
            let mut o: HashMap<String, u128> = HashMap::new();
            outflows(&client, &keys, &kc, s, "txlist", sb, &mut o).await;
            outflows(&client, &keys, &kc, s, "txlistinternal", sb, &mut o).await;
            o.remove(wr_store::addrs::zero());
            let tot: f64 = o.values().map(|&v| v as f64 / 1e18).sum();
            let mut top: Vec<(String, f64)> = o.iter().map(|(k, &v)| (k.clone(), v as f64 / 1e18)).collect();
            top.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
            top.truncate(8);
            println!("\n### ХАБ {s} — вихід {tot:.0} ETH до {} адрес", o.len());
            let mut dsts = Vec::new();
            for (to, eth) in &top {
                let label = if known.contains(to) { "back-to-network" }
                    else if is_contract(&client, &keys, &kc, to).await { "contract" }
                    else if nonce(&client, &keys, &kc, to).await >= nonce_cex { "exchange/hot" }
                    else { "external-EOA" };
                println!("   {eth:>10.0} ETH  {label:<15} {to}");
                dsts.push(json!({"to": to, "eth": eth.round(), "class": label}));
            }
            trace_out.push(json!({"hub": s, "out_eth": tot.round(), "uniq": o.len(), "top": dsts}));
        }
        serde_json::to_writer_pretty(
            File::create(format!("{root}/dataset/backfill/desk-hub-trace.json")).unwrap(),
            &json!({"start": start, "hubs": trace_out})).unwrap();
        eprintln!("збережено: desk-hub-trace.json");
        return;
    }

    // Режим CRAWL: багатохоповий BFS-граф від деску по native-ETH виходах. Розширюємо через
    // EOA-вузли, межа = біржа(nonce) / контракт (не розгортаємо). Env: MAX_HOPS(4) MAX_NODES(220) MIN_EDGE(300).
    if std::env::var("MODE").as_deref() == Ok("crawl") {
        use std::collections::VecDeque;
        let max_hops: usize = std::env::var("MAX_HOPS").ok().and_then(|s| s.parse().ok()).unwrap_or(4);
        let max_nodes: usize = std::env::var("MAX_NODES").ok().and_then(|s| s.parse().ok()).unwrap_or(220);
        let min_edge: f64 = std::env::var("MIN_EDGE").ok().and_then(|s| s.parse().ok()).unwrap_or(300.0);
        // seed-набір: SEED_FILE (по адресі на рядок) або дефолтний деск із cohort-analysis.json
        let seeds: Vec<String> = if let Ok(sf) = std::env::var("SEED_FILE") {
            fs::read_to_string(&sf).unwrap_or_default().lines()
                .map(|l| l.trim().to_lowercase()).filter(|l| l.len()==42 && l.starts_with("0x")).collect()
        } else { desk.clone() };
        eprintln!("[crawl] seed-вузлів: {}", seeds.len());
        let mut cls: HashMap<String, String> = HashMap::new();
        let mut internal: HashSet<String> = seeds.iter().cloned().collect();
        let mut hopmap: HashMap<String, usize> = internal.iter().map(|a| (a.clone(), 0)).collect();
        let mut q: VecDeque<(String, usize)> = seeds.iter().map(|a| (a.clone(), 0)).collect();
        let mut edges: Vec<(String, String, f64)> = Vec::new();
        let mut leak: HashMap<String, f64> = HashMap::new();     // біржа -> ETH
        let mut contracts: HashMap<String, f64> = HashMap::new();
        let mut fetched = 0;
        while let Some((a, hop)) = q.pop_front() {
            if hop >= max_hops || internal.len() >= max_nodes { continue; }
            let mut o: HashMap<String, u128> = HashMap::new();
            outflows(&client, &keys, &kc, &a, "txlist", sb, &mut o).await;
            outflows(&client, &keys, &kc, &a, "txlistinternal", sb, &mut o).await;
            fetched += 1;
            o.remove(wr_store::addrs::zero());
            for (to, wei) in &o {
                let eth = *wei as f64 / 1e18;
                if eth < min_edge || to == &a { continue; }
                edges.push((a.clone(), to.clone(), eth));
                if internal.contains(to) { continue; }
                // класифікувати (кеш)
                if !cls.contains_key(to) {
                    let lab = if is_contract(&client, &keys, &kc, to).await { "contract" }
                        else if nonce(&client, &keys, &kc, to).await >= nonce_cex { "exchange" }
                        else { "eoa" };
                    cls.insert(to.clone(), lab.to_string());
                }
                match cls[to].as_str() {
                    "exchange" => { *leak.entry(to.clone()).or_insert(0.0) += eth; }
                    "contract" => { *contracts.entry(to.clone()).or_insert(0.0) += eth; }
                    _ => { if internal.insert(to.clone()) {
                        hopmap.insert(to.clone(), hop + 1);
                        q.push_back((to.clone(), hop + 1));
                    }}
                }
            }
            if fetched % 20 == 0 { eprintln!("[crawl] fetched={fetched} internal={} q={} leak-адрес={}", internal.len(), q.len(), leak.len()); }
        }
        let total_leak: f64 = leak.values().sum();
        let total_contract: f64 = contracts.values().sum();
        let mut leakv: Vec<(&String, &f64)> = leak.iter().collect(); leakv.sort_by(|a, b| b.1.partial_cmp(a.1).unwrap());
        println!("=== CRAWL граф мережі (hops<={max_hops}, min_edge={min_edge} ETH) ===");
        println!("internal EOA-вузлів: {} | ребер: {} | fetched: {}", internal.len(), edges.len(), fetched);
        println!("біржовий вихід (leak): {total_leak:.0} ETH до {} біржових адрес", leak.len());
        println!("контрактний вихід: {total_contract:.0} ETH до {} контрактів", contracts.len());
        println!("частка кешування (leak / (leak+internal-circ)):");
        let internal_circ: f64 = edges.iter().filter(|(_, t, _)| internal.contains(t)).map(|(_, _, e)| e).sum();
        println!("  внутр.циркуляція {internal_circ:.0} ETH vs біржа {total_leak:.0} ETH -> leak={:.1}%",
            100.0 * total_leak / (total_leak + internal_circ).max(1.0));
        println!("топ біржові сливи:");
        for (a, e) in leakv.iter().take(10) { println!("   {:>10.0} ETH  {a}", e); }
        // розподіл за хопами
        let mut byhop: HashMap<usize, usize> = HashMap::new();
        for h in hopmap.values() { *byhop.entry(*h).or_insert(0) += 1; }
        let mut hv: Vec<(&usize, &usize)> = byhop.iter().collect(); hv.sort();
        println!("вузлів за хопами: {:?}", hv);
        let outp = std::env::var("OUT_NAME").unwrap_or_else(|_| "desk-network-graph.json".into());
        serde_json::to_writer_pretty(
            File::create(format!("{root}/dataset/backfill/{outp}")).unwrap(),
            &json!({"start": start, "internal_nodes": internal.iter().cloned().collect::<Vec<_>>(),
                    "edges": edges.iter().map(|(f,t,e)| json!([f,t,e.round()])).collect::<Vec<_>>(),
                    "exchange_leak": leakv.iter().map(|(a,e)| json!([a, e.round()])).collect::<Vec<_>>(),
                    "total_leak_eth": total_leak.round(), "internal_circ_eth": internal_circ.round(),
                    "nodes_by_hop": byhop})).unwrap();
        eprintln!("збережено: desk-network-graph.json");
        return;
    }

    let mut acc: HashMap<String, u128> = HashMap::new();
    for (i, m) in desk.iter().enumerate() {
        outflows(&client, &keys, &kc, m, "txlist", sb, &mut acc).await;
        outflows(&client, &keys, &kc, m, "txlistinternal", sb, &mut acc).await;
        if i % 10 == 0 { eprintln!("[{i}/{}] отримувачів: {}", desk.len(), acc.len()); }
    }
    // прибрати вихід на самих себе-нуль-адресу
    acc.remove(wr_store::addrs::zero());

    let total: f64 = acc.values().map(|&v| v as f64 / 1e18).sum();
    let mut top: Vec<(String, f64)> = acc.iter().map(|(k, &v)| (k.clone(), v as f64 / 1e18)).collect();
    top.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    top.truncate(topn);

    eprintln!("=== ТОП-{topn} отримувачів ETH від 39-деску (з {start}) ===");
    println!("разом вихід: {total:.0} ETH до {} унікальних адрес", acc.len());
    let mut dests = Vec::new();
    let mut cls_tot: HashMap<&str, f64> = HashMap::new();
    for (to, eth) in &top {
        let label = if deskset.contains(to) { "internal-desk" }
            else if cohort.contains(to) { "cohort-359" }
            else {
                let nc = nonce(&client, &keys, &kc, to).await;
                if nc >= nonce_cex { "exchange/hot" } else { "external" }
            };
        *cls_tot.entry(label).or_insert(0.0) += eth;
        println!("  {eth:>10.0} ETH  {label:<14} {to}");
        dests.push(json!({"to": to, "eth": eth.round(), "class": label}));
    }
    // класова сума по ВСІХ (не лише топ) — груба: тільки топ класифіковано, решта external-ish
    println!("\nкласи (по топ-{topn}):");
    let mut cv: Vec<(&&str, &f64)> = cls_tot.iter().collect();
    cv.sort_by(|a, b| b.1.partial_cmp(a.1).unwrap());
    for (c, v) in cv { println!("  {c:<14} {:.0} ETH", v); }

    serde_json::to_writer_pretty(
        File::create(format!("{root}/dataset/backfill/desk-flow.json")).unwrap(),
        &json!({"start": start, "desk_n": desk.len(), "total_out_eth": total.round(),
                "uniq_dests": acc.len(), "top": dests})).unwrap();
    eprintln!("збережено: desk-flow.json");
}
