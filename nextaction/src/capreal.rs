//! capreal — РЕАЛЬНИЙ капітал груп замість паперового.
//!
//! Проблема: досі капітал групи = Σ(net-холдинг × ціна). Це фікція одразу з двох причин:
//!   1) ціна може бути артефактом (див. token-prices/tokprices),
//!   2) навіть за правильної ціни ВИЙНЯТИ ці гроші нема звідки: у пулах COMP лежить $2.4М
//!      ліквідності й $8k обороту за добу — «холдинг на $8 млрд» неможливо продати.
//! Тому рахуємо ТРИ різні величини й не змішуємо їх:
//!   • ПАПІР        — net × ціна (як було, для порівняння)
//!   • РЕАЛІЗОВНЕ   — min(папір, LIQ_FRAC × ліквідність пулів токена) + нативний ETH за номіналом
//!                    (ETH глибокий, його не треба дисконтувати)
//!   • ЗАЛИТІ ГРОШІ — фактичний потік стейблів і WETH у пули/з пулів: скільки реальних грошей
//!                    група ВНЕСЛА в токени і скільки ВИЙНЯЛА (це і є «реальна ліквідність, залита
//!                    в токени», рахована з ончейн-переказів, а не з котирувань)
//!
//! Вхід: capital-groups.json (групи+члени), transfers.csv, token-prices.json (tokprices),
//!       token-liquidity.json (tokliq/GeckoTerminal), wallet-capital.jsonl (нативний ETH), token-supply.json
//! Вихід: dataset/backfill/cap-real.json + ОНОВЛЮЄ dataset/capital-groups.json (бекап .bak-real)
//! Args: ROOT. Env: LIQ_FRAC(0.10 — яку частку пулів реально можна забрати) WRITE(1)
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader};

fn is_junk(a: &str) -> bool { a.is_empty() || wr_store::addrs::is_junk_addr(a) }
/// «реальні гроші» у переказах: стейбли + загорнутий ETH
fn is_money(sym: &str) -> bool {
    let s = sym.to_uppercase();
    s.contains("USD") || matches!(s.as_str(), "DAI" | "FRAX" | "LUSD" | "TUSD" | "GHO" | "MIM" | "WETH" | "ETH")
}
/// не-альти, які не рахуємо як «позицію»
fn is_stable_or_wrapped(sym: &str) -> bool {
    let s = sym.to_uppercase();
    s.contains("USD") || s.contains("EUR") || s.contains("GBP")
        || matches!(s.as_str(), "DAI"|"FRAX"|"LUSD"|"TUSD"|"USTB"|"AUSD"|"GHO"|"MIM"|"WBTC"|"CBBTC"|"TBTC"|"WETH"|"WBETH"|"CBETH"|"WSTETH"|"STETH"|"RETH"|"WEETH"|"EETH")
        || s.ends_with("ON")
}

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let liq_frac: f64 = std::env::var("LIQ_FRAC").ok().and_then(|s| s.parse().ok()).unwrap_or(0.10);
    let write = std::env::var("WRITE").map(|v| v != "0").unwrap_or(true);
    let bf = format!("{root}/dataset/backfill");

    // --- групи + члени ---
    let cgpath = format!("{root}/dataset/capital-groups.json");
    let mut cg: Value = serde_json::from_reader(BufReader::new(File::open(&cgpath).expect("capital-groups.json"))).expect("cg parse");
    let groups = cg["groups"].as_array().cloned().unwrap_or_default();
    let mut gid: Vec<String> = Vec::new();
    let mut gsize: Vec<u64> = Vec::new();
    let mut gmem: Vec<HashSet<String>> = Vec::new();
    let mut member_of: HashMap<String, Vec<usize>> = HashMap::new();
    // ПОВНЕ членство — з group-membership.json (grouppart), а не з обрізаного members[] у застосунку:
    // саме через обрізку (2000 із 16868) капітал раніше рахувався по частині групи.
    let mut hop_of: HashMap<String, u64> = HashMap::new();
    let full: HashMap<String, Vec<String>> = serde_json::from_reader::<_, Value>(BufReader::new(File::open(format!("{bf}/group-membership.json")).expect("group-membership — спершу grouppart")))
        .ok().and_then(|v| v["wallets"].as_object().map(|m| {
            let mut h: HashMap<String, Vec<String>> = HashMap::new();
            for (w, x) in m {
                let w = w.to_lowercase();
                if let Some(hp) = x["hop"].as_u64() { hop_of.insert(w.clone(), hp); }
                h.entry(x["group"].as_str().unwrap_or("?").to_string()).or_default().push(w);
            }
            h })).unwrap_or_default();
    if !full.is_empty() {
        let mut ids: Vec<&String> = full.keys().collect();
        ids.sort();
        for g in ids {
            let ms = &full[g];
            gid.push(g.clone());
            gsize.push(ms.len() as u64);
            gmem.push(ms.iter().cloned().collect());
        }
        eprintln!("[capreal] членство з group-membership.json: {} груп", gid.len());
    } else {
    for (i, g) in groups.iter().enumerate() {
        gid.push(g["id"].as_str().unwrap_or("?").to_string());
        gsize.push(g["size"].as_u64().unwrap_or(0));
        let mut set = HashSet::new();
        for m in g["members"].as_array().cloned().unwrap_or_default() {
            let a = m["address"].as_str().or_else(|| m.as_str()).unwrap_or("").to_lowercase();
            if a.len() == 42 { set.insert(a); }
        }
        gmem.push(set);
    }
    }
    eprintln!("[capreal] груп: {} · унікальних членів: {}", gid.len(), member_of.len());

    // --- довідники ---
    let supply: serde_json::Map<String, Value> = serde_json::from_reader(BufReader::new(File::open(format!("{root}/dataset/token-supply.json")).unwrap())).unwrap_or_default();
    let mut sym_of: HashMap<String, String> = HashMap::new();
    for (c, v) in &supply { sym_of.insert(c.to_lowercase(), v["symbol"].as_str().unwrap_or("").to_string()); }
    let tp: Value = serde_json::from_reader(BufReader::new(File::open(format!("{bf}/token-prices.json")).expect("token-prices — спершу tokprices"))).unwrap();
    let mut px: HashMap<String, f64> = tp["tokens"].as_object().map(|m| m.iter().filter_map(|(k, v)| v["usd"].as_f64().map(|u| (k.to_lowercase(), u))).collect()).unwrap_or_default();
    let onchain: serde_json::Map<String, Value> = serde_json::from_reader(BufReader::new(File::open(format!("{bf}/onchain-prices.json")).unwrap())).unwrap_or_default();
    for (k, v) in &onchain {
        let k = k.to_lowercase();
        if px.contains_key(&k) { continue; }
        if let Some(u) = v["usd"].as_f64() { if u > 0.0 && u < 10_000.0 { px.insert(k, u); } }
    }
    let liq: Value = serde_json::from_reader(BufReader::new(File::open(format!("{bf}/token-liquidity.json")).expect("token-liquidity — спершу tokliq"))).unwrap();
    let lq: HashMap<String, (f64, f64, String)> = liq["tokens"].as_object().map(|m| m.iter().map(|(k, v)| (k.to_lowercase(),
        (v["reserveUsd"].as_f64().unwrap_or(0.0), v["vol24"].as_f64().unwrap_or(0.0), v["symbol"].as_str().unwrap_or("").to_string()))).collect()).unwrap_or_default();
    let mut kind: HashMap<String, String> = HashMap::new();
    if let Ok(v) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(File::open(format!("{bf}/wallet-kind.json")).unwrap())) {
        for (a, k) in &v { kind.insert(a.to_lowercase(), k.as_str().unwrap_or("").to_string()); }
    }
    let is_contract = |a: &str| kind.get(a).map(|s| s == "contract").unwrap_or(false);
    let lbl: serde_json::Map<String, Value> = serde_json::from_reader(BufReader::new(File::open(format!("{root}/dataset/labels-cache.json")).unwrap())).unwrap_or_default();
    let is_cex = |a: &str| matches!(kind.get(a).map(|s| s.as_str()), Some("exchange") | Some("otc"))
        || lbl.get(a).and_then(|x| x["name"].as_str()).map(|n| { let l = n.to_lowercase();
            ["binance","crypto.com","okx","gate","coinbase","kraken","bybit","kucoin","bitget","bitfinex","huobi","mexc","gemini","korbit","upbit","bithumb","uphold","bittrex","poloniex","paribu","btcturk","bingx","gopax","exchange","hot wallet","deposit"].iter().any(|c| l.contains(c)) }).unwrap_or(false);
    // ІНФРАСТРУКТУРА НЕ Є КАПІТАЛОМ ГРУПИ: біржові гаманці й контракти (пули/роутери/мости) тримають
    // чужі гроші. Без цього фільтра «нативний ETH» групи A = $8.85млрд, з яких більшість — Bitfinex
    // hot wallet і контракти. KEEP_INFRA=1 повертає стару (паперову) поведінку.
    let keep_infra = std::env::var("KEEP_INFRA").map(|v| v == "1").unwrap_or(false);
    // код на ланцюжку (ethbal фаза 2) + сам факт «це контракт токена» — найнадійніші ознаки:
    // мітки й wallet-kind пропускали навіть контракт WETH (2.3М ETH у «капіталі групи»).
    let onchain_code: HashSet<String> = serde_json::from_reader::<_, Value>(BufReader::new(File::open(format!("{bf}/eth-balances.json")).expect("eth-balances")))
        .ok().and_then(|v| v["wallets"].as_object().map(|m| m.iter().filter(|(_, x)| x["code"] == json!(true)).map(|(k, _)| k.to_lowercase()).collect())).unwrap_or_default();
    let is_token_contract = |a: &str| sym_of.contains_key(a) || px.contains_key(a);
    let is_infra = |a: &str| !keep_infra && (is_cex(a) || is_contract(a) || onchain_code.contains(a) || is_token_contract(a));
    // членство після фільтра: капітал групи формують лише її ВЛАСНІ (EOA) гаманці
    let all_members: HashSet<String> = gmem.iter().flatten().cloned().collect();
    let mut excluded: Vec<usize> = vec![0; gid.len()];
    let mut eoa_mem: Vec<HashSet<String>> = Vec::new();
    for (i, set) in gmem.iter().enumerate() {
        let mut s = HashSet::new();
        for a in set {
            if is_infra(a) { excluded[i] += 1; continue; }
            member_of.entry(a.clone()).or_default().push(i);
            s.insert(a.clone());
        }
        eoa_mem.push(s);
    }
    let gmem_orig = gmem;
    let gmem = eoa_mem;
    eprintln!("[capreal] членів після фільтра інфраструктури: {} (виключено біржі/контракти: {})",
        member_of.len(), excluded.iter().sum::<usize>());

    // нативний ETH: РЕАЛЬНИЙ баланс (ethbal/Etherscan). ETH глибоко ліквідний → за номіналом.
    let mut eth_real: HashMap<String, f64> = HashMap::new();
    if let Ok(v) = serde_json::from_reader::<_, Value>(BufReader::new(File::open(format!("{bf}/eth-balances.json")).expect("eth-balances — спершу ethbal"))) {
        if let Some(m) = v["wallets"].as_object() {
            for (a, x) in m { eth_real.insert(a.to_lowercase(), x["usd"].as_f64().unwrap_or(0.0)); }
        }
    }
    // стара flow-net оцінка — лише для порівняння «папір vs факт»
    let mut eth_usd: HashMap<String, f64> = HashMap::new();
    if let Ok(f) = File::open(format!("{bf}/wallet-capital.jsonl")) {
        for line in BufReader::new(f).lines().map_while(Result::ok) {
            if let Ok(o) = serde_json::from_str::<Value>(&line) {
                if let Some(w) = o["w"].as_str() {
                    let w = w.to_lowercase();
                    if all_members.contains(&w) { eth_usd.insert(w, o["eth_usd"].as_f64().unwrap_or(0.0)); }
                }
            }
        }
    }

    // --- один прохід по transfers ---
    let mut net: HashMap<(usize, String), f64> = HashMap::new();     // (група, токен) -> net-кількість
    let mut money_in: Vec<f64> = vec![0.0; gid.len()];               // стейбли/WETH → у пули (куплено)
    let mut money_out: Vec<f64> = vec![0.0; gid.len()];              // стейбли/WETH ← з пулів (продано)
    let mut rows = 0u64;
    let f = BufReader::with_capacity(1 << 20, File::open(format!("{bf}/transfers.csv")).expect("transfers"));
    for line in f.lines().map_while(Result::ok) {
        let mut it = line.splitn(6, ',');
        let w = it.next().unwrap_or(""); let _c = it.next(); let d = it.next().unwrap_or(""); let _ts = it.next();
        let tok = it.next().unwrap_or(""); let rest = it.next().unwrap_or("");
        if w == "wallet" || (d != "i" && d != "o") { continue; }
        let wl = w.to_lowercase();
        let Some(gs) = member_of.get(&wl) else { continue };
        let sym = rest.split(',').next().unwrap_or("");
        let mut rit = line.rsplitn(4, ',');
        let _h = rit.next();
        let amt: f64 = rit.next().unwrap_or("0").parse().unwrap_or(0.0);
        let cp = rit.next().unwrap_or("").to_lowercase();
        if amt <= 0.0 || is_junk(&cp) { continue; }
        rows += 1;
        let tl = tok.to_lowercase();
        if is_money(sym) {
            // реальні гроші: рух ЧЕРЕЗ пули = залито/вийнято
            if is_contract(&cp) {
                let usd = if sym.eq_ignore_ascii_case("WETH") || sym.eq_ignore_ascii_case("ETH") {
                    amt * px.get(wr_store::addrs::weth()).copied().unwrap_or(tp["eth"].as_f64().unwrap_or(0.0))
                } else { amt };   // стейбл ≈ $1
                for g in gs { if d == "o" { money_in[*g] += usd } else { money_out[*g] += usd } }
            }
        } else if !is_stable_or_wrapped(sym) {
            let s = if d == "i" { amt } else { -amt };
            for g in gs { *net.entry((*g, tl.clone())).or_insert(0.0) += s; }
        }
    }
    eprintln!("[capreal] рядків членів опрацьовано: {rows}");

    // --- підсумки по групах ---
    let mut report: Vec<Value> = Vec::new();
    for (i, id) in gid.iter().enumerate() {
        let mut assets: Vec<Value> = Vec::new();
        let (mut paper, mut realiz) = (0.0f64, 0.0f64);
        for ((g, t), n) in net.iter() {
            if *g != i || *n <= 0.0 { continue; }
            let Some(p) = px.get(t) else { continue };
            let pu = n * p;
            if pu < 1000.0 { continue; }
            let (res, vol, lsym) = lq.get(t).cloned().unwrap_or((0.0, 0.0, String::new()));
            let sym = if !lsym.is_empty() { lsym } else { sym_of.get(t).cloned().unwrap_or_default() };
            // реалізовне: не більше, ніж LIQ_FRAC пулів; якщо ліквідності не знайдено — 0 (нема куди продати)
            // Фаза 0.2: по-активна реалізовність — зі СЛОВНИКА (той самий `pu.min(frac*res)`) → байт-паритет.
            let r = wr_dict::realizable_usd(pu, res, liq_frac);
            paper += pu; realiz += r;
            assets.push(json!({"symbol": sym, "token": t, "amount": n, "paperUsd": pu, "reserveUsd": res,
                               "vol24": vol, "realizableUsd": r, "daysOfVolume": if vol > 0.0 { pu / vol } else { -1.0 }}));
        }
        assets.sort_by(|a, b| b["paperUsd"].as_f64().unwrap().partial_cmp(&a["paperUsd"].as_f64().unwrap()).unwrap());
        let eth: f64 = gmem[i].iter().filter_map(|w| eth_real.get(w)).sum();
        // ядро (hop 0-2, тісно звʼязані) проти периферії (hop ≥3 — часто разові контрагенти)
        let eth_core: f64 = gmem[i].iter().filter(|w| hop_of.get(*w).map(|h| *h <= 2).unwrap_or(false)).filter_map(|w| eth_real.get(w)).sum();
        let core_n = gmem[i].iter().filter(|w| hop_of.get(*w).map(|h| *h <= 2).unwrap_or(false)).count();
        let eth_est: f64 = gmem[i].iter().filter_map(|w| eth_usd.get(w)).sum();
        let eth_all: f64 = gmem_orig[i].iter().filter_map(|w| eth_real.get(w)).sum();
        let real_total = eth + realiz;
        report.push(json!({
            "id": id, "size": gsize[i], "membersUsed": gmem[i].len(),
            "membersExcludedInfra": excluded[i], "ethUsdExcludedInfra": eth_all - eth,
            "coreMembers": core_n, "coreEthUsd": eth_core, "peripheryEthUsd": eth - eth_core,
            "paperUsd": paper, "realizableUsd": realiz, "nativeEthUsd": eth, "nativeEthUsdFlowEstimate": eth_est, "realTotalUsd": real_total,
            "moneyInUsd": money_in[i], "moneyOutUsd": money_out[i], "moneyNetUsd": money_in[i] - money_out[i],
            "liqFrac": liq_frac, "assets": assets,
        }));
    }
    report.sort_by(|a, b| b["realTotalUsd"].as_f64().unwrap().partial_cmp(&a["realTotalUsd"].as_f64().unwrap()).unwrap());
    std::fs::write(format!("{bf}/cap-real.json"), serde_json::to_string_pretty(&json!({"liqFrac": liq_frac, "groups": report})).unwrap()).ok();

    eprintln!("[capreal] ===== КАПІТАЛ ГРУП: ПАПІР vs РЕАЛЬНІСТЬ (частка пулів {:.0}%) =====", liq_frac * 100.0);
    eprintln!("{:<6} {:>8} {:>16} {:>16} {:>16} {:>16} {:>16}", "група", "членів", "папір-токени", "реалізовне", "нативний ETH", "РЕАЛЬНО разом", "залито грошей");
    for r in &report {
        eprintln!("{:<6} {:>8} {:>16.0} {:>16.0} {:>16.0} {:>16.0} {:>16.0}",
            r["id"].as_str().unwrap_or("?"), r["membersUsed"].as_u64().unwrap_or(0),
            r["paperUsd"].as_f64().unwrap_or(0.0), r["realizableUsd"].as_f64().unwrap_or(0.0),
            r["nativeEthUsd"].as_f64().unwrap_or(0.0), r["realTotalUsd"].as_f64().unwrap_or(0.0),
            r["moneyInUsd"].as_f64().unwrap_or(0.0));
    }
    eprintln!("[capreal] --- розріз ядро(hop0-2) / периферія(hop≥3) ---");
    for r in report.iter().take(5) {
        eprintln!("   {:<8} ядро {:>5} гам ${:>14.0} · периферія ${:>14.0}", r["id"].as_str().unwrap_or("?"),
            r["coreMembers"].as_u64().unwrap_or(0), r["coreEthUsd"].as_f64().unwrap_or(0.0), r["peripheryEthUsd"].as_f64().unwrap_or(0.0));
    }
    if let Some(top) = report.first() {
        eprintln!("[capreal] --- {} : топ-активи (папір → реалізовне) ---", top["id"].as_str().unwrap_or(""));
        for a in top["assets"].as_array().cloned().unwrap_or_default().iter().take(12) {
            eprintln!("     {:<10} папір ${:>14.0} · пули ${:>12.0} · оборот24 ${:>12.0} · реально ${:>12.0} · днів обороту {:.0}",
                a["symbol"].as_str().unwrap_or("?"), a["paperUsd"].as_f64().unwrap_or(0.0), a["reserveUsd"].as_f64().unwrap_or(0.0),
                a["vol24"].as_f64().unwrap_or(0.0), a["realizableUsd"].as_f64().unwrap_or(0.0), a["daysOfVolume"].as_f64().unwrap_or(-1.0));
        }
    }

    // --- запис у застосунок ---
    if write {
        std::fs::copy(&cgpath, format!("{cgpath}.bak-real")).ok();
        let by_id: HashMap<String, &Value> = report.iter().map(|r| (r["id"].as_str().unwrap_or("").to_string(), r)).collect();
        if let Some(gs) = cg["groups"].as_array_mut() {
            for g in gs.iter_mut() {
                let id = g["id"].as_str().unwrap_or("").to_string();
                let Some(r) = by_id.get(&id) else { continue };
                let paper_old = g["totalUsd"].as_f64().unwrap_or(0.0);
                g["totalUsdPaperOld"] = json!(paper_old);
                g["totalUsd"] = json!(r["realTotalUsd"].as_f64().unwrap_or(0.0));
                g["totalUsdPaper"] = r["paperUsd"].clone();
                g["realizableUsd"] = r["realizableUsd"].clone();
                g["nativeEthUsd"] = r["nativeEthUsd"].clone();
                g["moneyInUsd"] = r["moneyInUsd"].clone();
                g["moneyOutUsd"] = r["moneyOutUsd"].clone();
                g["moneyNetUsd"] = r["moneyNetUsd"].clone();
                g["capitalBasis"] = json!(format!("реальний = нативний ETH + min(папір, {:.0}% ліквідності пулів); папір лишено окремо", liq_frac * 100.0));
                // анотуємо активи ліквідністю
                let amap: HashMap<String, Value> = r["assets"].as_array().cloned().unwrap_or_default().into_iter()
                    .map(|a| (a["symbol"].as_str().unwrap_or("").to_string(), a)).collect();
                if let Some(as_) = g["assets"].as_array_mut() {
                    for a in as_.iter_mut() {
                        let s = a["symbol"].as_str().unwrap_or("").to_string();
                        if let Some(src) = amap.get(&s) {
                            a["usdPaper"] = a["usd"].clone();
                            a["usd"] = src["realizableUsd"].clone();
                            a["reserveUsd"] = src["reserveUsd"].clone();
                            a["vol24"] = src["vol24"].clone();
                            a["daysOfVolume"] = src["daysOfVolume"].clone();
                        } else { a["usdPaper"] = a["usd"].clone(); a["usd"] = json!(0.0); a["reserveUsd"] = json!(0.0); }
                    }
                }
            }
        }
        cg["capitalBasis"] = json!("totalUsd = РЕАЛЬНИЙ капітал (нативний ETH + реалізовна частина токенів за ліквідністю пулів); totalUsdPaper = стара паперова оцінка");
        std::fs::write(&cgpath, serde_json::to_string_pretty(&cg).unwrap()).ok();
        eprintln!("[capreal] capital-groups.json оновлено (бекап .bak-real)");
    }
    eprintln!("[capreal] збережено cap-real.json");
}

#[cfg(test)]
mod parity_tests {
    /// БАЙТ-ПАРИТЕТ (Фаза 0.2): по-активна реалізовність `pu.min(liq_frac*res)`
    /// ≡ wr_dict::realizable_usd(pu, res, liq_frac) бітово (той самий вираз).
    #[test]
    fn realizable_bit_identical() {
        let fracs = [0.05f64, 0.10, 0.20];
        let papers = [0.0f64, 1000.0, 250_000.0, 1e9, 1.23e12];
        let liqs = [0.0f64, 1.0, 5e5, 1e9, 7.7e11];
        for &frac in &fracs { for &pu in &papers { for &res in &liqs {
            assert_eq!(pu.min(frac * res).to_bits(), wr_dict::realizable_usd(pu, res, frac).to_bits());
        }}}
    }
}
