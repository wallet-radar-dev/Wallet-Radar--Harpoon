//! emission: хто (гаманець І поведінкова ГРУПА) тримає найбільший % емісії (частку
//! total_supply) по активах. Групи — з groups-universe.json (communities/desks = тісні
//! co-holding спільноти незв'язаних гаманців + archetypes = широкі поведінкові кластери).
//! Метрика: share = holding / total_supply, clamp<=1 (flow-net переоцінює при churn).
//! ЛИШЕ chain=eth (supply тягнуто з eth.blockscout → інші чейни змішали б бриджі з eth-supply).
//! Вихід: emission-wallets.json (топ індив. холдери/токен) + emission-groups.json (топ груп).
//! Без RPC: transfers.csv + token-supply.json + groups-universe.json + wallet-capital-classified (kind).

use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};

fn envf(k: &str, d: f64) -> f64 { std::env::var(k).ok().and_then(|s| s.parse().ok()).unwrap_or(d) }

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let min_group_share = envf("MIN_GROUP_SHARE", 0.005); // 0.5% емісії — поріг для звіту
    let min_wallet_share = envf("MIN_WALLET_SHARE", 0.005);
    let topn: usize = std::env::var("TOPN").ok().and_then(|s| s.parse().ok()).unwrap_or(40);

    // 1) supply: contract(lower) -> (human_supply, symbol)
    let sv: Value = serde_json::from_reader(BufReader::new(
        File::open(format!("{root}/dataset/token-supply.json")).expect("token-supply.json"),
    )).expect("parse supply");
    let mut supply: HashMap<String, (f64, String)> = HashMap::new();
    if let Some(m) = sv.as_object() {
        for (c, v) in m {
            let dec = v["decimals"].as_u64().unwrap_or(18) as i32;
            let raw: f64 = v["supply"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0.0);
            if raw <= 0.0 { continue; }
            let hs = raw / 10f64.powi(dec);
            if hs <= 0.0 { continue; }
            supply.insert(c.to_lowercase(), (hs, v["symbol"].as_str().unwrap_or("").to_string()));
        }
    }
    eprintln!("[emission] токенів із supply: {}", supply.len());

    // 2) kind-кеш (wallet-kind.json): виключаємо контракти/біржі — не контролери емісії
    let mut kind: HashMap<String, String> = HashMap::new();
    if let Ok(v) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(
        File::open(format!("{root}/dataset/backfill/wallet-kind.json")).expect("wallet-kind.json (запусти capital-classify з ADDR_FILE)"),
    )) {
        for (a, k) in v {
            kind.insert(a.to_lowercase(), k.as_str().unwrap_or("").to_string());
        }
    }
    // які kind виключати (env EXCLUDE_KINDS). Дефолт — усе не-EOA. Щоб включити DEX-контракти
    // й OTC-дески в групи (частина операції) — EXCLUDE_KINDS=exchange (лише централізовані біржі).
    let excl: std::collections::HashSet<String> = std::env::var("EXCLUDE_KINDS")
        .unwrap_or_else(|_| "contract,exchange,otc".into())
        .split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    let is_infra = |w: &str| kind.get(w).map(|k| excl.contains(k)).unwrap_or(false);
    eprintln!("[emission] kind-записів: {}; виключаємо kinds: {:?}", kind.len(), excl);

    // 3) групи = ТЕМПОРАЛЬНА co-movement (coordination wallet-groups.json: groups[].wallets).
    //    Схожий набір активів + тайминг входу/торгів/виходу; гаманці незв'язані.
    let gf = std::env::var("GROUPS_FILE").unwrap_or_else(|_| format!("{root}/dataset/wallet-groups.json"));
    let gu: Value = serde_json::from_reader(BufReader::new(File::open(&gf).expect("wallet-groups.json"))).expect("parse groups");
    let mut w2g: HashMap<String, Vec<String>> = HashMap::new();
    let mut gtype: HashMap<String, (String, usize, Value)> = HashMap::new();
    if let Some(arr) = gu["groups"].as_array() {
        for (i, g) in arr.iter().enumerate() {
            let members = g["wallets"].as_array().cloned().unwrap_or_default();
            let size = g["size"].as_u64().unwrap_or(members.len() as u64) as usize;
            let label = format!("coord:{i}");
            gtype.insert(label.clone(), ("coord".into(), size, g["topSharedTokens"].clone()));
            for m in &members {
                if let Some(a) = m.as_str() {
                    w2g.entry(a.to_lowercase()).or_default().push(label.clone());
                }
            }
        }
    }
    eprintln!("[emission] темпоральних груп: {}; гаманців у групах: {}", gtype.len(), w2g.len());

    // 4) прохід transfers: per (wallet, contract) net holding, ЛИШЕ chain=eth & contract у supply
    let mut hold: HashMap<(String, String), f64> = HashMap::new();
    let mut rows = 0u64;
    if let Ok(f) = File::open(format!("{root}/dataset/backfill/transfers.csv")) {
        for line in BufReader::new(f).lines().map_while(Result::ok) {
            rows += 1;
            let mut fr = line.split(',');
            let wallet = fr.next().unwrap_or("");
            if wallet.is_empty() || wallet == "wallet" { continue; }
            let chain = fr.next().unwrap_or("");
            if chain != "eth" { continue; }
            let dir = fr.next().unwrap_or("");
            let _ts = fr.next();
            let token = fr.next().unwrap_or("");
            if dir != "i" && dir != "o" { continue; }
            let tk = token.to_lowercase();
            if !supply.contains_key(&tk) { continue; }
            let mut rr = line.rsplitn(3, ',');
            let _hash = rr.next();
            let amt: f64 = rr.next().unwrap_or("0").parse().unwrap_or(0.0);
            if amt == 0.0 { continue; }
            let signed = if dir == "i" { amt } else { -amt };
            *hold.entry((wallet.to_lowercase(), tk)).or_insert(0.0) += signed;
        }
    }
    eprintln!("[emission] рядків: {rows}; (wallet,token) позицій: {}", hold.len());

    // 5) per-token: індив. холдери (share clamp<=1) + coverage
    // token -> Vec<(wallet, holding, share)>
    let mut tok_holders: HashMap<String, Vec<(String, f64, f64)>> = HashMap::new();
    // group aggregation: (group_label, token) -> (sum_share, n_holders)
    let mut grp: HashMap<(String, String), (f64, u32)> = HashMap::new();
    for ((w, tk), &n) in &hold {
        if n <= 0.0 { continue; }
        if is_infra(w) { continue; } // контракти/біржі не контролюють емісію
        let (hs, _sym) = supply.get(tk).unwrap();
        // Фаза 0.2: частка емісії — зі СЛОВНИКА (hs>0 гарантовано завантажувачем) → байт-паритет.
        let share = wr_dict::emission_share(n, *hs).min(1.0);
        if share <= 0.0 { continue; }
        tok_holders.entry(tk.clone()).or_default().push((w.clone(), n, share));
        if let Some(groups) = w2g.get(w) {
            for gl in groups {
                let e = grp.entry((gl.clone(), tk.clone())).or_insert((0.0, 0));
                e.0 += share;
                e.1 += 1;
            }
        }
    }

    // 6) emission-wallets.json — per token, топ холдери + coverage
    let mut tokens_out: Vec<Value> = Vec::new();
    let mut wallet_leader: Vec<(String, String, String, f64, f64)> = Vec::new(); // sym,contract,wallet,share,holding
    for (tk, mut hs_list) in tok_holders.clone() {
        let (hs, sym) = supply.get(&tk).unwrap().clone();
        hs_list.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap());
        let coverage: f64 = hs_list.iter().map(|x| x.2).sum::<f64>().min(1.0);
        let top: Vec<Value> = hs_list.iter().take(10).map(|(w, h, s)| json!({
            "w": w, "holding": h, "share": (s*1e6).round()/1e6,
            "kind": kind.get(w).cloned().unwrap_or_default()
        })).collect();
        for (w, h, s) in hs_list.iter().take(3) {
            if *s >= min_wallet_share { wallet_leader.push((sym.clone(), tk.clone(), w.clone(), *s, *h)); }
        }
        tokens_out.push(json!({
            "contract": tk, "symbol": sym, "supply": hs,
            "tracked_coverage": (coverage*1e4).round()/1e4,
            "n_tracked_holders": hs_list.len(), "top_holders": top
        }));
    }
    tokens_out.sort_by(|a, b| b["tracked_coverage"].as_f64().unwrap().partial_cmp(&a["tracked_coverage"].as_f64().unwrap()).unwrap());
    fs::write(format!("{root}/dataset/backfill/emission-wallets.json"),
        serde_json::to_string_pretty(&json!({"tokens": tokens_out})).unwrap()).ok();

    // 7) emission-groups.json — (group, token) за часткою емісії
    let mut grp_rows: Vec<Value> = Vec::new();
    for ((gl, tk), (sum_share, nh)) in &grp {
        let gs = sum_share.min(1.0);
        if gs < min_group_share { continue; }
        let (gt, size, shared) = gtype.get(gl).cloned().unwrap_or_default();
        let (hs, sym) = supply.get(tk).unwrap().clone();
        // топ-члени групи по цьому токену
        let mut mem: Vec<&(String, f64, f64)> = tok_holders.get(tk).map(|v| v.iter()
            .filter(|(w, _, _)| w2g.get(w).map(|gs2| gs2.contains(gl)).unwrap_or(false)).collect()).unwrap_or_default();
        mem.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap());
        let top_mem: Vec<Value> = mem.iter().take(6).map(|(w, _h, s)| json!({
            "w": w, "share": (s*1e6).round()/1e6, "kind": kind.get(w).cloned().unwrap_or_default()
        })).collect();
        grp_rows.push(json!({
            "group": gl, "group_type": gt, "group_size": size, "shared_tokens": shared,
            "symbol": sym, "contract": tk, "supply": hs,
            "group_emission_share": (gs*1e6).round()/1e6, "n_holders_in_group": nh,
            "top_members": top_mem
        }));
    }
    grp_rows.sort_by(|a, b| b["group_emission_share"].as_f64().unwrap().partial_cmp(&a["group_emission_share"].as_f64().unwrap()).unwrap());
    fs::write(format!("{root}/dataset/backfill/emission-groups.json"),
        serde_json::to_string_pretty(&json!({"pairs": grp_rows})).unwrap()).ok();

    // 8) звіт
    wallet_leader.sort_by(|a, b| b.3.partial_cmp(&a.3).unwrap());
    eprintln!("\n=== ТОП-{topn} ГРУП за часткою емісії (group,token) ===", );
    eprintln!("{:<26} {:>7} {:<10} {:>8} {:>6}  members", "group", "size", "token", "share%", "nHold");
    for r in grp_rows.iter().take(topn) {
        let ex: Vec<String> = r["top_members"].as_array().unwrap().iter().take(3)
            .map(|m| format!("{}{}", &m["w"].as_str().unwrap()[..10],
                if m["kind"].as_str().unwrap_or("")=="contract" {"(C)"} else {""})).collect();
        eprintln!("{:<26} {:>7} {:<10} {:>7.2}% {:>6}  {}",
            r["group"].as_str().unwrap(), r["group_size"].as_u64().unwrap(),
            r["symbol"].as_str().unwrap(), r["group_emission_share"].as_f64().unwrap()*100.0,
            r["n_holders_in_group"].as_u64().unwrap(), ex.join(","));
    }
    eprintln!("\n=== ТОП-25 ІНДИВ. холдерів за часткою емісії (wallet,token) ===");
    for (sym, _c, w, s, _h) in wallet_leader.iter().take(25) {
        let k = kind.get(w).cloned().unwrap_or_default();
        eprintln!("  {:>7.2}%  {:<10} {} {}", s*100.0, sym, w, if k=="contract"{"(CONTRACT!)"}else{""});
    }
    eprintln!("\nгруп-пар >{}%: {}; токен-звітів: {}", min_group_share*100.0, grp_rows.len(), tokens_out.len());
    eprintln!("вихід: emission-wallets.json | emission-groups.json");
}

#[cfg(test)]
mod parity_tests {
    /// БАЙТ-ПАРИТЕТ (Фаза 0.2): патерн `(x/s).min(1.0)` (emission.rs/cascade.rs) і plain `x/s`
    /// (avoid.rs) тотожні `emission_share(x,s)` бітово для s>0 (усі завантажувачі гарантують s>0).
    #[test]
    fn emission_share_bit_identical() {
        let sups: [f64; 6] = [1.0, 1.5, 1e6, 528_786_477.77, 1e18, 183_987_824_878.0];
        let xs: [f64; 6] = [0.0, 1.0, 250_000.0, 1e9, 9.9e17, 1.23456789e12];
        for &s in &sups {
            for &x in &xs {
                assert_eq!((x / s).to_bits(), wr_dict::emission_share(x, s).to_bits());
                assert_eq!((x / s).min(1.0).to_bits(), wr_dict::emission_share(x, s).min(1.0).to_bits());
            }
        }
    }
}
