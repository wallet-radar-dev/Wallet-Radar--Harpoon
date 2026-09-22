//! groupbook — книга позицій групи капіталу: скільки купили, по чому, скільки продали, по чому.
//!
//! Питання: по якій ціні найбільші групи набирали ETH і BTC, скільки набрали, по чому виходили —
//! і те саме по альтах, де вони тримають найбільшу частку емісії.
//!
//! Що рахуємо на кожну пару група×актив:
//!   * КУПЛЕНО    — сума входів у токенах і в грошах за ціною ДНЯ кожного входу;
//!                  середня ціна входу = гроші / токени (VWAP, а не середнє по днях:
//!                  великий вхід має важити більше за дрібний).
//!   * ПРОДАНО    — те саме на виходах, середня ціна виходу.
//!   * РЕАЛІЗОВАНО — продано_грошей − продано_токенів × середня_ціна_входу.
//!   * ЛИШИЛОСЬ   — сальдо потоків × поточна ціна.
//!
//! Чесність цифр, три застереження просто в даних:
//!   1. Внутрішньогрупові перекази викинуто з обох боків — рух між своїми не є ні купівлею,
//!      ні продажем, інакше та сама монета «купувалась» і «продавалась» десятки разів.
//!   2. Вихід ≠ продаж: переказ на біржу чи на свою нову адресу поза групою тут теж
//!      порахується як вихід. Тому колонка зветься «вихід», а не «прибуток».
//!   3. Сальдо з потоків систематично бреше (неповна історія), тож де є реальний баланс
//!      із ланцюга (eth-balances.json для ETH), береться ВІН, а потокове — лише довідково.
//!
//! BTC на Ethereum = WBTC. Справжній біткоїн сюди не входить: зв'язати BTC-адреси з цими
//! гаманцями нема чим (у датасеті 39 BTC-адрес, і вони не з цих груп).
//!
//! Вихід: dataset/backfill/groupbook.json + таблиця у stdout.
//! Args: ROOT. Env: GROUPS(HUB,NET,NET-WEAK) TOP_ALTS(8) MIN_USD(250000)

use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, Write};

fn envs(k: &str, d: &str) -> String {
    std::env::var(k).ok().filter(|s| !s.is_empty()).unwrap_or_else(|| d.to_string())
}
fn envf(k: &str, d: f64) -> f64 { envs(k, "").parse().unwrap_or(d) }

fn days_from_civil(y: i64, m: i64, d: i64) -> i32 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146097 + doe - 719468) as i32
}
fn parse_day(ts: &str) -> Option<i32> {
    if ts.len() < 10 { return None; }
    Some(days_from_civil(ts[0..4].parse().ok()?, ts[5..7].parse().ok()?, ts[8..10].parse().ok()?))
}
fn civil(z: i32) -> (i64, i64, i64) {
    let z = z as i64 + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}
fn dstr(z: i32) -> String { let (y, m, d) = civil(z); format!("{y:04}-{m:02}-{d:02}") }

#[derive(Default, Clone)]
struct Book {
    // ВАЛОВИЙ обіг — не позиція, а прогін. Лишаємо, бо саме він показує транзит:
    // у HUB валом проходить 28 млн ETH при залишку 1.2 млн.
    gross_in: f64, gross_out: f64,
    first: i32, last: i32, ops_in: u64, ops_out: u64,
}

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".to_string());
    let b = format!("{root}/dataset/backfill");
    let groups: HashSet<String> = envs("GROUPS", "HUB,NET,NET-WEAK")
        .split(',').map(|s| s.trim().to_string()).collect();
    let top_alts: usize = envf("TOP_ALTS", 8.0) as usize;
    let min_usd = envf("MIN_USD", 250_000.0);

    // членство
    let mem: Value = serde_json::from_reader(BufReader::new(
        File::open(format!("{b}/group-membership.json")).expect("group-membership.json"))).unwrap();
    let mut wg: HashMap<String, String> = HashMap::new();
    for (w, v) in mem["wallets"].as_object().unwrap() {
        let g = v["group"].as_str().unwrap_or("");
        if groups.contains(g) { wg.insert(w.to_ascii_lowercase(), g.to_string()); }
    }
    eprintln!("[groupbook] груп {} | гаманців {}", groups.len(), wg.len());

    // емісія + ціни
    let mut supply: HashMap<String, (f64, String)> = HashMap::new();
    if let Ok(f) = File::open(format!("{b}/token-supply.json")) {
        if let Ok(v) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(f)) {
            for (c, x) in v {
                if let Some(s) = x["supply"].as_f64() {
                    if s > 0.0 { supply.insert(c.to_lowercase(), (s, x["symbol"].as_str().unwrap_or("").into())); }
                }
            }
        }
    }
    if let Ok(f) = File::open(format!("{b}/tokenpnl.jsonl")) {
        for l in BufReader::new(f).lines().map_while(Result::ok) {
            let d: Value = match serde_json::from_str(&l) { Ok(v) => v, Err(_) => continue };
            if let (Some(c), Some(s)) = (d["contract"].as_str(), d["supply"].as_f64()) {
                if s > 0.0 {
                    supply.entry(c.to_lowercase())
                        .or_insert((s, d["symbol"].as_str().unwrap_or("").to_string()));
                }
            }
        }
    }
    let ph: Value = serde_json::from_reader(BufReader::new(
        File::open(format!("{b}/token-prices-hist.json")).expect("token-prices-hist.json"))).unwrap();
    let mut prices: HashMap<String, Vec<(i32, f64)>> = HashMap::new();
    if let Some(o) = ph["tokens"].as_object() {
        for (c, x) in o {
            let Some(arr) = x["prices"].as_array() else { continue };
            let mut v: Vec<(i32, f64)> = arr.iter().filter_map(|p| {
                let a = p.as_array()?;
                Some(((a[0].as_i64()? / 86400) as i32, a[1].as_f64()?))
            }).filter(|(_, p)| *p > 0.0).collect();
            v.sort_by_key(|x| x.0); v.dedup_by_key(|x| x.0);
            if !v.is_empty() { prices.insert(c.to_lowercase(), v); }
        }
    }
    // нативний ETH іде під ключем "eth"; WETH коштує те саме, тож за потреби позичає ряд
    let weth = wr_store::addrs::weth();
    if !prices.contains_key(weth) {
        if let Some(e) = prices.get("eth").cloned() { prices.insert(weth.into(), e); }
    }
    if !prices.contains_key("eth") {
        if let Some(e) = prices.get(weth).cloned() { prices.insert("eth".into(), e); }
    }
    let price_at = |c: &str, d: i32| -> Option<f64> {
        let p = prices.get(c)?;
        Some(match p.binary_search_by_key(&d, |x| x.0) {
            Ok(i) => p[i].1,
            Err(0) => p[0].1,
            Err(i) => p[i - 1].1,
        })
    };

    // потік
    let mut book: HashMap<(String, String), Book> = HashMap::new();
    let mut sym_of: HashMap<String, String> = HashMap::new();
    // Згортання ПО ДНЯХ — ядро методу. Група ганяє ті самі монети через пули: вихід у
    // пул і повернення назад того ж дня давали «купив 1000, продав 998» замість «набрав 2».
    // Валові потоки через це показували 118 млн WETH у NET — більше, ніж WETH існує.
    // Купівлею вважаємо додатне ДЕННЕ сальдо групи, продажем — від'ємне.
    let mut day_net: HashMap<(String, String, i32), (f64, f64)> = HashMap::new();
    let mut seen: HashSet<u64> = HashSet::with_capacity(8_000_000);
    let (mut read, mut used, mut intern) = (0u64, 0u64, 0u64);

    for name in ["transfers.csv", "transfers-grp.csv", "transfers-native.csv", "transfers-es.csv"] {
        // Живий файл ПЛЮС .zst-архіви: після прокрутки старша історія лежить стисненою.
        let parts = backfill::csvin::readers(&format!("{b}/{name}"));
        if parts.is_empty() { eprintln!("[groupbook] нема {name}"); continue }
        let mut raw: Vec<u8> = Vec::with_capacity(256);
        let mut fu = 0u64;
        for mut rd in parts {
        loop {
            raw.clear();
            // Читаємо БАЙТАМИ: read_line вимагає коректний UTF-8 і на першому ж непридатному
            // байті повертає помилку, а код на ній зупиняв розбір УСЬОГО файлу — мовчки.
            match rd.read_until(10, &mut raw) { Ok(0) => break, Ok(_) => {}, Err(_) => continue }
            read += 1;
            let line = String::from_utf8_lossy(&raw);
            let s = line.trim_end();
            if s.is_empty() || s.starts_with("wallet,") { continue; }
            let mut it = s.splitn(6, ',');
            let (Some(w), Some(_ch), Some(dir), Some(ts), Some(tok)) =
                (it.next(), it.next(), it.next(), it.next(), it.next()) else { continue };
            let Some(rest) = it.next() else { continue };
            let mut rit = rest.rsplitn(4, ',');
            let (Some(txh), Some(amt), Some(cp), Some(sym)) =
                (rit.next(), rit.next(), rit.next(), rit.next()) else { continue };
            let Some(g) = wg.get(w) else { continue };
            if wg.get(cp) == Some(g) { intern += 1; continue; }
            let Some(day) = parse_day(ts) else { continue };
            let Ok(a) = amt.parse::<f64>() else { continue };
            if !a.is_finite() || a <= 0.0 { continue; }
            let key = tok.to_ascii_lowercase();
            if !prices.contains_key(&key) { continue; }
            let mut h: u64 = 0xcbf29ce484222325;
            for x in [txh, w, dir, key.as_str(), amt] {
                for byte in x.as_bytes() { h ^= *byte as u64; h = h.wrapping_mul(0x100000001b3); }
                h ^= 0xff; h = h.wrapping_mul(0x100000001b3);
            }
            if !seen.insert(h) { continue; }
            if price_at(&key, day).is_none() { continue }
            sym_of.entry(key.clone()).or_insert_with(|| sym.to_string());
            let e = book.entry((g.clone(), key.clone())).or_default();
            if e.first == 0 || day < e.first { e.first = day }
            if day > e.last { e.last = day }
            let d = day_net.entry((g.clone(), key.clone(), day)).or_insert((0.0, 0.0));
            if dir == "i" { e.gross_in += a; e.ops_in += 1; d.0 += a }
            else { e.gross_out += a; e.ops_out += 1; d.1 += a }
            used += 1; fu += 1;
        }
        }
        eprintln!("[groupbook] {name}: узято {fu}");
    }
    eprintln!("[groupbook] рядків {read}, узято {used}, внутрішніх {intern}, пар {}", book.len());

    // реальні баланси ETH — потокове сальдо для ETH систематично бреше
    let mut eth_real: HashMap<String, f64> = HashMap::new();
    if let Ok(f) = File::open(format!("{b}/eth-balances.json")) {
        if let Ok(v) = serde_json::from_reader::<_, Value>(BufReader::new(f)) {
            if let Some(o) = v["wallets"].as_object() {
                for (a, x) in o {
                    if let Some(g) = wg.get(&a.to_ascii_lowercase()) {
                        *eth_real.entry(g.clone()).or_insert(0.0) += x["eth"].as_f64().unwrap_or(0.0);
                    }
                }
            }
        }
    }

    // денні сальдо -> набір і розвантаження
    let mut agg: HashMap<(String, String), (f64, f64, f64, f64)> = HashMap::new(); // (куп.ток, куп.$, прод.ток, прод.$)
    for ((g, c, d), (i, o)) in &day_net {
        let net = i - o;
        if net == 0.0 { continue }
        let px = price_at(c, *d).unwrap_or(0.0);
        let e = agg.entry((g.clone(), c.clone())).or_insert((0.0, 0.0, 0.0, 0.0));
        if net > 0.0 { e.0 += net; e.1 += net * px } else { e.2 += -net; e.3 += -net * px }
    }

    // Реальна частка емісії з ЛАНЦЮГА (netassets: balanceOf членів мережі). Потокове
    // сальдо для цього не годиться — воно систематично завищене неповною історією.
    // Гранулярність різна: netassets рахує мережу цілком (HUB+NET), тож поле й зветься
    // networkwide, а не приписується окремій групі.
    let mut real_share: HashMap<String, (f64, bool)> = HashMap::new();
    if let Ok(f) = File::open(format!("{b}/netassets.json")) {
        if let Ok(v) = serde_json::from_reader::<_, Value>(BufReader::new(f)) {
            for a in v["assets"].as_array().cloned().unwrap_or_default() {
                if let Some(c) = a["contract"].as_str() {
                    real_share.insert(c.to_lowercase(), (
                        a["emissionPctHeldByNetwork"].as_f64().unwrap_or(0.0),
                        a["thin"].as_bool().unwrap_or(false)));
                }
            }
        }
    }
    eprintln!("[groupbook] реальних часток із ланцюга: {}", real_share.len());

    let now_day = *prices.get("eth").and_then(|v| v.last()).map(|(d, _)| d).unwrap_or(&0);
    let mut rows: Vec<Value> = vec![];
    for ((g, c), bk) in &book {
        let (bt, bu, st, su) = agg.get(&(g.clone(), c.clone())).cloned().unwrap_or((0.0, 0.0, 0.0, 0.0));
        if bu < min_usd && su < min_usd { continue; }
        let sym = sym_of.get(c).cloned().unwrap_or_default();
        let vwap_in = if bt > 0.0 { bu / bt } else { 0.0 };
        let vwap_out = if st > 0.0 { su / st } else { 0.0 };
        // реалізовано лише на проданій частині, за середньою ціною набору
        let realized = su - st * vwap_in;
        let net = bt - st;
        let px_now = price_at(c, now_day).unwrap_or(0.0);
        let (sup, _) = supply.get(c).cloned().unwrap_or((0.0, String::new()));
        // частка емісії з потоків — довідково: сальдо систематично бреше, і коли воно
        // виходить за межі емісії, це доказ, що історія неповна, а не що група володіє всім
        let share = if sup > 0.0 { wr_dict::emission_pct(net, sup) } else { 0.0 };
        // Набрати більше, ніж існує емісії, неможливо. Якщо так вийшло — це не позиція,
        // а обіг: ті самі монети пройшли через групу багато разів через пули. Число
        // позначаємо непридатним ТУТ, а не залишаємо читачеві помічати оком.
        let churn_suspect = sup > 0.0 && (bt > sup || net.abs() > sup);
        let (real_pct, real_thin) = real_share.get(c).cloned().unwrap_or((f64::NAN, false));
        rows.push(json!({
            "group": g, "symbol": sym, "contract": c,
            "churnSuspect": churn_suspect,
            "emissionPctRealNetworkwide": if real_pct.is_nan() { Value::Null } else { json!(real_pct) },
            "realThin": real_thin,
            "boughtTokens": bt, "boughtUsd": bu, "avgBuyPrice": vwap_in,
            "soldTokens": st, "soldUsd": su, "avgSellPrice": vwap_out,
            "realizedUsd": realized,
            "netTokensFlow": net, "netUsdNow": net * px_now, "priceNow": px_now,
            "emissionSharePctFlow": share,
            "shareImpossible": sup > 0.0 && (share > 100.0 || share < 0.0),
            "grossInTokens": bk.gross_in, "grossOutTokens": bk.gross_out,
            "churnRatio": if bt > 0.0 { bk.gross_in / bt } else { 0.0 },
            "firstDay": dstr(bk.first), "lastDay": dstr(bk.last),
            "opsIn": bk.ops_in, "opsOut": bk.ops_out
        }));
    }
    rows.sort_by(|a, b| b["boughtUsd"].as_f64().unwrap().partial_cmp(&a["boughtUsd"].as_f64().unwrap()).unwrap());

    let doc = json!({
        "generated": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs(),
        "dictVersion": wr_dict::dict_version(),
        "note": "вихід ≠ продаж (переказ на біржу теж вихід); BTC = WBTC, справжній біткоїн не покритий",
        "ethRealBalance": eth_real, "positions": rows
    });
    File::create(format!("{b}/groupbook.json")).unwrap()
        .write_all(serde_json::to_string_pretty(&doc).unwrap().as_bytes()).unwrap();

    let getf = |v: &Value, k: &str| v[k].as_f64().unwrap_or(0.0);
    println!("\nРЕАЛЬНИЙ ETH НА ЛАНЦЮГУ (не з потоків):");
    for (g, v) in &eth_real { println!("  {g:<10} {v:>14.0} ETH"); }
    for g in &groups {
        let mut gr: Vec<&Value> = rows.iter().filter(|r| r["group"].as_str() == Some(g)).collect();
        if gr.is_empty() { continue }
        gr.sort_by(|a, b| getf(b, "boughtUsd").partial_cmp(&getf(a, "boughtUsd")).unwrap());
        println!("\n=== {g} ===");
        println!("{:<8}{:>15}{:>11}{:>15}{:>11}{:>15}{:>8}{:>9}", "актив", "набрано", "серед.вхід", "здано", "серед.вихід", "реалізовано", "оберт", "%емісії");
        for r in gr.iter().take(top_alts + 3) {
            let imp = if r["churnSuspect"].as_bool().unwrap_or(false) { "обіг!" } else { "" };
            println!("{:<8}{:>15.2}{:>11.2}{:>15.2}{:>11.2}{:>15.0}{:>8.0}{:>8.2}{}",
                r["symbol"].as_str().unwrap_or(""), getf(r, "boughtTokens"), getf(r, "avgBuyPrice"),
                getf(r, "soldTokens"), getf(r, "avgSellPrice"), getf(r, "realizedUsd"),
                getf(r, "churnRatio"), getf(r, "emissionSharePctFlow"), imp);
        }
    }
    println!("\nJSON: {b}/groupbook.json");
}
