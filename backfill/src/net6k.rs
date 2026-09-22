//! net6k — портфель мережі 6864 гаманців: що тримали, що мають зараз, яка частка емісії,
//! і скільки з цього вийшло грошима.
//!
//! Один прохід по `txfull-<chain>.jsonl`. Рахує окремо:
//!   * ERC-20 (`k=tok`) — у кожного токена свої `decimals`, тож нормалізуємо по рядку;
//!   * нативний ETH (`k=ext|int`) — інша семантика, у спільну суму не мішаємо.
//!     (Змішування давало 19.8 трлн «ETH» при 120 млн існуючих — токенний `v` ділився на 1e18.)
//!
//! ДЕДУП локальний, у межах гаманця: `txfull` пише рядки одного гаманця суцільним блоком,
//! а дублі виникають від перекриття вікон пагінації (~0.5%), тобто завжди сусідні.
//! Глобальна множина на 60+ млн ключів з'їдала 11 ГБ і клала ВМ.
//!
//! ПИЛ І ОТРУЄННЯ: перекази нижче MIN_TOK від емісії ігноруються — інакше в «холдери»
//! потрапляє адресний спам (двійники з нульовими сумами).
//!
//! Вихід: dataset/backfill/net6k-positions.json + звіт у stderr.
//! Args: ROOT [WALLET_FILE]. Env: CHAIN(1) TOP(30) MIN_USD(0)
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};

fn envs(k: &str, d: &str) -> String { std::env::var(k).unwrap_or_else(|_| d.to_string()) }
fn envn<T: std::str::FromStr>(k: &str, d: T) -> T {
    std::env::var(k).ok().and_then(|s| s.parse().ok()).unwrap_or(d)
}

/// Дешево дістає значення рядкового поля без повного розбору JSON — на 60+ млн рядків
/// різниця відчутна, а нам спершу треба лише відсіяти чужі гаманці.
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
    let chain: u32 = envn("CHAIN", 1);
    let top: usize = envn("TOP", 30);
    let pair_min: f64 = envn("PAIR_MIN_USD", 10000.0);
    let wfile = std::env::args().nth(2).unwrap_or_else(|| format!("{bf}/net-members.txt"));

    // --- члени мережі ---
    let members: HashSet<String> = BufReader::new(File::open(&wfile).expect("wallet file"))
        .lines().map_while(Result::ok)
        .map(|l| l.trim().to_lowercase())
        .filter(|l| l.len() == 42)
        .collect();
    eprintln!("[net6k] членів мережі: {}", members.len());

    // --- ціни ---
    let px_raw: Value = serde_json::from_reader(BufReader::new(
        File::open(format!("{bf}/token-prices.json")).expect("token-prices.json"))).unwrap();
    let eth_usd = px_raw["eth"].as_f64().unwrap_or(0.0);
    let mut price: HashMap<String, f64> = HashMap::new();
    let mut symbol: HashMap<String, String> = HashMap::new();
    if let Some(o) = px_raw["tokens"].as_object() {
        for (c, v) in o {
            let c = c.to_lowercase();
            price.insert(c.clone(), v["usd"].as_f64().unwrap_or(0.0));
            symbol.insert(c, v["symbol"].as_str().unwrap_or("").to_string());
        }
    }

    // --- емісія (supply) з tokenpnl ---
    let mut supply: HashMap<String, f64> = HashMap::new();
    if let Ok(f) = File::open(format!("{bf}/tokenpnl.jsonl")) {
        for line in BufReader::new(f).lines().map_while(Result::ok) {
            let Ok(d) = serde_json::from_str::<Value>(&line) else { continue };
            let c = d["contract"].as_str().unwrap_or("").to_lowercase();
            if c.is_empty() { continue }
            if let Some(s) = d["supply"].as_f64() { if s > 0.0 { supply.insert(c.clone(), s); } }
            if !symbol.contains_key(&c) {
                if let Some(s) = d["symbol"].as_str() { symbol.insert(c, s.to_string()); }
            }
        }
    }
    // token-supply.json — добране `toksupply` для тих, кого не було в tokenpnl (WETH, WBTC,
    // MKR, wstETH…). Саме там найбільші позиції, тож без цього частка емісії лишалась «?»
    // рівно там, де вона найцікавіша.
    if let Ok(f) = File::open(format!("{bf}/token-supply.json")) {
        if let Ok(m) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(f)) {
            for (c, v) in m {
                let c = c.to_lowercase();
                if let Some(s) = v["supply"].as_f64() {
                    if s > 0.0 { supply.entry(c.clone()).or_insert(s); }
                }
                if let Some(s) = v["symbol"].as_str() {
                    if !s.is_empty() { symbol.entry(c).or_insert_with(|| s.to_string()); }
                }
            }
        }
    }
    eprintln!("[net6k] цін: {} · supply: {}", price.len(), supply.len());

    // --- прохід по базі ---
    let path = if chain == 1 { format!("{bf}/txfull-1.jsonl") } else { format!("{bf}/txfull-{chain}.jsonl") };
    let f = File::open(&path).expect("txfull");
    let rdr = BufReader::with_capacity(1 << 22, f);

    let mut pos: HashMap<(u32, String), f64> = HashMap::new();   // (гаманець, токен) -> баланс
    let mut tin: HashMap<String, f64> = HashMap::new();          // токен -> усього надійшло
    let mut tout: HashMap<String, f64> = HashMap::new();
    let mut eth_in = 0f64;
    let mut eth_out = 0f64;
    // окремо по видах і по гаманцях — щоб було видно, ЗВІДКИ береться сума, а не лише підсумок
    let mut eth_in_ext = 0f64;
    let mut eth_in_int = 0f64;
    let mut eth_out_ext = 0f64;
    let mut eth_out_int = 0f64;
    let mut w_eth: HashMap<u32, (f64, f64)> = HashMap::new();
    let mut widx: HashMap<String, u32> = HashMap::new();
    let mut seen: HashSet<u64> = HashSet::new();
    let mut cur_w = String::new();
    let (mut n, mut kept) = (0u64, 0u64);

    for line in rdr.lines().map_while(Result::ok) {
        n += 1;
        if n % 10_000_000 == 0 { eprintln!("    ...{n} рядків, наших {kept}"); }
        let Some(w) = field(&line, "w") else { continue };
        if !members.contains(w) { continue }
        if w != cur_w { cur_w = w.to_string(); seen.clear(); }

        let Ok(t) = serde_json::from_str::<Value>(&line) else { continue };
        // дешевий відбиток рядка замість зберігання самих полів
        let mut h: u64 = 1469598103934665603;
        for s in [t["h"].as_str().unwrap_or(""), t["k"].as_str().unwrap_or(""),
                  t["f"].as_str().unwrap_or(""), t["t"].as_str().unwrap_or(""),
                  t["v"].as_str().unwrap_or("")] {
            for b in s.as_bytes() { h ^= *b as u64; h = h.wrapping_mul(1099511628211); }
        }
        if !seen.insert(h) { continue }
        kept += 1;

        let next = widx.len() as u32;
        let wi = *widx.entry(cur_w.clone()).or_insert(next);
        let kind = t["k"].as_str().unwrap_or("");
        let frm = t["f"].as_str().unwrap_or("").to_lowercase();
        let to = t["t"].as_str().unwrap_or("").to_lowercase();
        let raw: f64 = t["v"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0.0);
        if raw <= 0.0 { continue }

        if kind == "tok" {
            let c = t["tok"].as_str().unwrap_or("").to_lowercase();
            if c.is_empty() { continue }
            let dec: i32 = t["dec"].as_str().and_then(|s| s.parse().ok()).unwrap_or(18);
            let amt = raw / 10f64.powi(dec);
            if amt <= 0.0 { continue }
            if to == cur_w {
                *pos.entry((wi, c.clone())).or_insert(0.0) += amt;
                *tin.entry(c).or_insert(0.0) += amt;
            } else if frm == cur_w {
                *pos.entry((wi, c.clone())).or_insert(0.0) -= amt;
                *tout.entry(c).or_insert(0.0) += amt;
            }
        } else if kind == "ext" || kind == "int" {
            let v = raw / 1e18;
            let e = w_eth.entry(wi).or_insert((0.0, 0.0));
            if to == cur_w {
                eth_in += v; e.0 += v;
                if kind == "ext" { eth_in_ext += v } else { eth_in_int += v }
            } else if frm == cur_w {
                eth_out += v; e.1 += v;
                if kind == "ext" { eth_out_ext += v } else { eth_out_int += v }
            }
        }
    }
    eprintln!("[net6k] прочитано {n} рядків, наших {kept}");
    drop(seen);

    // --- агрегація по токенах ---
    let mut held: HashMap<String, f64> = HashMap::new();
    let mut holders: HashMap<String, u32> = HashMap::new();
    for ((_, c), bal) in &pos {
        if *bal > 1e-12 {
            *held.entry(c.clone()).or_insert(0.0) += *bal;
            *holders.entry(c.clone()).or_insert(0) += 1;
        }
    }

    let mut rows: Vec<Value> = Vec::new();
    for (c, bal) in &held {
        let p = price.get(c).copied().unwrap_or(0.0);
        let sup = supply.get(c).copied();
        let ever_in = tin.get(c).copied().unwrap_or(0.0);
        let ever_out = tout.get(c).copied().unwrap_or(0.0);
        rows.push(json!({
            "token": c,
            "symbol": symbol.get(c).cloned().unwrap_or_default(),
            "heldNow": bal,
            "usdNow": bal * p,
            "price": p,
            "holders": holders.get(c).copied().unwrap_or(0),
            "everIn": ever_in,
            "everOut": ever_out,
            // скільки вийшло з мережі у грошах за НИНІШНЬОЮ ціною — не виручка,
            // бо ціна на момент виходу була інша; це лише масштаб реалізації
            "outUsdAtNow": ever_out * p,
            "supply": sup,
            // Фаза 0.2: частка емісії — зі СЛОВНИКА. У map s завжди >0 (supply вставляється
            // лише при s>0), тож emission_pct(x,s) ≡ 100*x/s бітово → байт-паритет.
            "emissionPct": sup.map(|s| wr_dict::emission_pct(*bal, s)),
            "peakEmissionPct": sup.map(|s| wr_dict::emission_pct(ever_in, s)),
        }));
    }
    rows.sort_by(|a, b| b["usdNow"].as_f64().unwrap_or(0.0)
        .partial_cmp(&a["usdNow"].as_f64().unwrap_or(0.0)).unwrap());

    let usd_now: f64 = rows.iter().filter_map(|r| r["usdNow"].as_f64()).sum();
    let out_usd: f64 = rows.iter().filter_map(|r| r["outUsdAtNow"].as_f64()).sum();
    let eth_net = eth_in - eth_out;

    let out = json!({
        "members": members.len(),
        "chain": chain,
        "ethIn": eth_in, "ethOut": eth_out, "ethNet": eth_net,
        "ethNetUsd": eth_net * eth_usd, "ethPrice": eth_usd,
        "tokensHeld": rows.len(),
        "usdNowTotal": usd_now,
        "outUsdAtNowTotal": out_usd,
        "rows": rows,
    });
    let outp = format!("{bf}/net6k-positions.json");
    let mut fo = File::create(&outp).unwrap();
    let _ = fo.write_all(serde_json::to_string(&out).unwrap().as_bytes());

    // ПАРИ (гаманець, токен) для звірки з реальними балансами.
    // Баланс, порахований як «сума входів мінус виходів», СИСТЕМАТИЧНО бреше: історія в базі
    // неповна, тож частина виходів відсутня. Звірка показала розрив до ×11 (genesis-адреса UNI:
    // 147 млн проти реальних 12.5 млн) і випадки «наш 1.9 млн проти реальних 0».
    // Тому підсумкові суми треба будувати на `tokenbalance`, а ці пари — вхід для перевірки.
    let pairp = format!("{bf}/net6k-pairs.jsonl");
    if let Ok(mut pf) = File::create(&pairp) {
        let inv: HashMap<u32, &String> = widx.iter().map(|(w, i)| (*i, w)).collect();
        let mut cnt = 0usize;
        for ((wi, c), bal) in &pos {
            if *bal <= 0.0 { continue }
            let p = price.get(c).copied().unwrap_or(0.0);
            // Поріг у доларах, а не «дрібне відкидаємо»: PAIR_MIN_USD=0 дає повний перебір.
            // Токени без ціни все одно експортуємо (p=0) — їх частку емісії рахуємо в штуках.
            if p > 0.0 && p * *bal < pair_min { continue }
            if let Some(w) = inv.get(wi) {
                let _ = writeln!(pf, "{}", json!({"w": w, "tok": c, "est": bal, "usdEst": p * *bal,
                    "sym": symbol.get(c).cloned().unwrap_or_default()}));
                cnt += 1;
            }
        }
        eprintln!("[net6k] пар для звірки балансів: {cnt} → {pairp}");
    }

    eprintln!("\n=== ПІДСУМОК ===");
    eprintln!("ETH: отримано {:.1} · відправлено {:.1} · НЕТТО {:.1} (${:.0})",
        eth_in, eth_out, eth_net, eth_net * eth_usd);
    // Розбивка ext/int — головна діагностика: у txlistinternal лежать перекази, породжені
    // контрактами, і для гаманця, що працює через проксі/роутери, вони НЕ дублюють ext,
    // але роздувають оборот. Якщо int сумірний з ext — підсумок читати як оборот, не баланс.
    eprintln!("  з них ext: +{:.1} / -{:.1} · int: +{:.1} / -{:.1}",
        eth_in_ext, eth_out_ext, eth_in_int, eth_out_int);
    let mut we: Vec<(&u32, &(f64, f64))> = w_eth.iter().collect();
    we.sort_by(|a, b| (b.1.0 - b.1.1).partial_cmp(&(a.1.0 - a.1.1)).unwrap());
    let inv: HashMap<u32, &String> = widx.iter().map(|(w, i)| (*i, w)).collect();
    eprintln!("  ТОП-8 гаманців за нетто-ETH (де накопичується сума):");
    for (i, (wi, (i_, o_))) in we.iter().take(8).enumerate() {
        eprintln!("    {}. {} нетто {:.0} ETH (+{:.0} / -{:.0})",
            i + 1, inv.get(wi).map(|s| s.as_str()).unwrap_or("?"), i_ - o_, i_, o_);
    }
    eprintln!("токенів у портфелі: {} · вартість зараз ${:.0}", rows.len(), usd_now);
    eprintln!("вийшло з мережі (за нинішньою ціною): ${:.0}", out_usd);
    eprintln!("\n=== ТОП-{top} ПОЗИЦІЙ ЗАРАЗ ===");
    for r in rows.iter().take(top) {
        let e = r["emissionPct"].as_f64().map(|x| format!("{x:.2}%")).unwrap_or("?".into());
        let pk = r["peakEmissionPct"].as_f64().map(|x| format!("{x:.2}%")).unwrap_or("?".into());
        eprintln!("  {:<12} ${:>14.0} · зараз {} емісії (пік {}) · холдерів {}",
            r["symbol"].as_str().unwrap_or(""), r["usdNow"].as_f64().unwrap_or(0.0),
            e, pk, r["holders"].as_u64().unwrap_or(0));
    }
    eprintln!("\n[net6k] збережено {outp}");
    let _ = fs::metadata(&outp);
}

#[cfg(test)]
mod parity_tests {
    /// БАЙТ-ПАРИТЕТ (Фаза 0.2): emissionPct/peakEmissionPct на wr_dict::emission_pct.
    /// У коді s завжди >0 (у map), тож emission_pct(x,s) ≡ старий `100*x/s` бітово.
    #[test]
    fn emission_pct_bit_identical() {
        let sups: [f64; 6] = [1.0, 1.5, 1e6, 528_786_477.77, 1e18, 183_987_824_878.0];
        let bals: [f64; 6] = [0.0, 1.0, 250_000.0, 1e9, 9.9e17, 1.23456789e12];
        for &s in &sups {
            for &b in &bals {
                assert_eq!((100.0 * b / s).to_bits(), wr_dict::emission_pct(b, s).to_bits(),
                    "розбіжність b={b}, s={s}");
            }
        }
    }
}
