//! dexswap — ФАКТ угоди: купівля чи продаж, відновлені з ОБОХ ніг свопу в одній транзакції.
//!
//! НАВІЩО. Усе, що ми досі називали «злив у DEX», було здогадом: `cexsignal` і `sellstudy`
//! рахують продажем будь-який вихідний переказ альта НА КОНТРАКТ. Але переказ на контракт —
//! це не угода, а лише її слід, і напрямок з нього не читається. Тим часом справжня угода
//! лежить у нас на диску цілком: у свопі гаманець в ОДНІЙ транзакції віддає одне й отримує
//! інше, тож і сторона угоди, і ЦІНА ВИКОНАННЯ відновлюються точно. Перевірено на 400k
//! рядків: 100 359 зі 154 437 пар (гаманець, tx) мають обидві ноги.
//!
//! Запитів до Etherscan НЕ робить: квота не витрачається, працює з наявного CSV.
//!
//! ПАСТКА, закладена в метод — ТРАНЗИТ. У вибірці трапляється `i:CREAM(37.378) o:CREAM(37.378)`:
//! гаманець прийняв і віддав те саме в одній транзакції (арбітраж, роутинг). Якщо рахувати
//! ПЕРЕКАЗИ, кожен такий прохід стане фантомним «продажем», і стрічка роздується. Тому
//! сторона визначається за НЕТТО в межах транзакції, а токени з нульовим нетто відкидаються.
//!
//! ДРУГА ПАСТКА — ЦІНА. Токен-ціни брехливі (onchain-prices дає COMP ×2350), тож USD тут
//! рахується НЕ з ціни проданого токена, а з ДРУГОЇ НОГИ: стейбл = $1, WETH = історична
//! денна ціна. Ціна самого активу виходить наслідком угоди, а не входом у розрахунок.
//!
//! ТРЕТЯ ПАСТКА — СИМВОЛИ. Поле symbol у CSV не можна брати за істину: там трапляється
//! токен-спам на кшталт «Visit https://get-usdc.org to claim rewards». Квотні активи
//! визначаються ВИКЛЮЧНО за адресою контракту.
//!
//! СЛІПА ЗОНА цієї версії, названа вголос: своп за НАТИВНИЙ ETH має другу ногу в
//! transfers-native.csv (там є і txlistinternal), а не тут. Такі транзакції зараховуються
//! в лічильник `half_no_quote` і НЕ вигадуються — join нативних ніг це наступний крок.
//!
//! Виходи: dataset/backfill/trades.jsonl         — журнал угод (факти)
//!         dataset/backfill/trades-positions.json — по (група, актив): скільки вже злито
//! Args: ROOT. Env: MIN_USD(0) CAP(1000000) LIMIT(0=усе) MAX_TS_SKEW_D(3)
//!       OUT_DIR — куди писати результат (типово поруч із даними). Знадобилось, бо
//!       диск даних /mnt/nvme-disk забитий на 100%: писати результат нікуди, а читати
//!       вхід звідти можна й далі.
use backfill::csvin;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::File;
use std::io::{BufRead, BufReader, Write};

// ─────────────────────── квотні активи (за АДРЕСОЮ, не за символом) ───────────────────────
/// Стейбли: ціна = $1. Похибка прив'язки (±0.3%) на порядки менша за похибку будь-якої
/// токен-ціни, тож окремий фід для них був би точністю заради виду.
// Квотні активи (стейбли + WETH) переїхали у SQLite (`tokens`). Тут лишились
// лише функції доступу: перелік стейблів може рости, і рік тому його розширення
// означало правку коду й перезбирання — тепер це рядок у базі.



/// WBTC свідомо НЕ квотний: це позиція, а не одиниця розрахунку. Інакше продаж альта за
/// WBTC зник би зі стрічки як «обмін грошей на гроші».
#[derive(PartialEq, Clone, Copy)]
enum Cls {
    Stable,
    Weth,
    Base,
}

fn classify(tok: &str, stables: &HashSet<String>) -> Cls {
    if tok == wr_store::addrs::weth() {
        Cls::Weth
    } else if stables.contains(tok) {
        Cls::Stable
    } else {
        Cls::Base
    }
}

fn envn<T: std::str::FromStr>(k: &str, d: T) -> T {
    std::env::var(k).ok().and_then(|s| s.parse().ok()).unwrap_or(d)
}

/// Розбір рядка CSV, стійкий до КОМ У СИМВОЛІ.
///
/// `split(',')` тут не годиться: symbol — довільний текст із токена, і кома в ньому
/// зсуває всі наступні поля. Тому перші п'ять полів беруться зліва, останні три —
/// справа, а symbol лишається тим, що посередині (він нам і не потрібен точним).
/// Схема: wallet,chain,dir,ts,token,symbol,counterparty,amount,tx_hash
struct Row<'a> {
    w: &'a str,
    chain: &'a str,
    dir: &'a str,
    ts: &'a str,
    tok: &'a str,
    sym: &'a str,
    amt: &'a str,
    tx: &'a str,
}

fn parse(line: &str) -> Option<Row<'_>> {
    let mut l = line.splitn(6, ',');
    let w = l.next()?;
    let chain = l.next()?;
    let dir = l.next()?;
    let ts = l.next()?;
    let tok = l.next()?;
    let rest = l.next()?; // symbol,counterparty,amount,tx_hash
    let mut r = rest.rsplitn(4, ',');
    let tx = r.next()?;
    let amt = r.next()?;
    let _cp = r.next()?;
    let sym = r.next()?;
    Some(Row { w, chain, dir, ts, tok, sym, amt, tx })
}

/// «2023-09-05T04:52:11.000000Z» → unix. Формат у CSV один на всі рядки, тож розбір
/// ручний: chrono тут коштував би мільярд алокацій на повному проході.
fn ts_unix(s: &str) -> Option<i64> {
    if s.len() < 19 {
        return None;
    }
    let n = |a: usize, z: usize| -> Option<i64> { s.get(a..z)?.parse().ok() };
    let (y, mo, d) = (n(0, 4)?, n(5, 7)?, n(8, 10)?);
    let (h, mi, se) = (n(11, 13)?, n(14, 16)?, n(17, 19)?);
    // днів від епохи — цивільний алгоритм Говарда Хіннанта
    let y2 = if mo <= 2 { y - 1 } else { y };
    let era = if y2 >= 0 { y2 } else { y2 - 399 } / 400;
    let yoe = y2 - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(days * 86400 + h * 3600 + mi * 60 + se)
}

/// Денний ряд ціни WETH з token-prices-hist.json (DefiLlama). Пошук — найближча точка
/// НЕ ПІЗНІШЕ моменту угоди; якщо вона далі за MAX_TS_SKEW_D — ціни немає, і USD лишається
/// незаповненим. Підставити сусідню ціну через рік означало б вигадати число.
struct EthPx {
    pts: Vec<(i64, f64)>,
    skew: i64,
}
impl EthPx {
    fn at(&self, ts: i64) -> Option<f64> {
        let i = self.pts.partition_point(|(t, _)| *t <= ts);
        if i == 0 {
            return None;
        }
        let (t, p) = self.pts[i - 1];
        if ts - t > self.skew {
            None
        } else {
            Some(p)
        }
    }
}

struct Bucket {
    ts: i64,
    chain: String,
    legs: Vec<(String, String, bool, f64)>,
    seen: HashSet<u64>,
}

fn hash_leg(tok: &str, dir: bool, amt: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    tok.hash(&mut h);
    dir.hash(&mut h);
    amt.hash(&mut h);
    h.finish()
}

#[derive(Default)]
struct Stats {
    trades: u64,
    buys: u64,
    sells: u64,
    usd_buy: f64,
    usd_sell: f64,
    transit: u64,
    half_no_quote: u64,
    single_leg: u64,
    fx: u64,
    multi: u64,
    mixed: u64,
    no_px: u64,
    below_min: u64,
    dup_leg: u64,
    bad_row: u64,
}

#[derive(Default)]
struct Pos {
    sym: String,
    buy_usd: f64,
    sell_usd: f64,
    buy_u: f64,
    sell_u: f64,
    n: u64,
    first: i64,
    last: i64,
}

fn r2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}
fn r4(x: f64) -> f64 {
    (x * 10000.0).round() / 10000.0
}

struct Ctx {
    // Власні String, а не позики: перелік тепер приходить із бази,
    // а не з масиву-константи з часом життя 'static.
    stables: HashSet<String>,
    sym_of: HashMap<String, String>,
    eth: EthPx,
    groups: HashMap<String, String>,
    min_usd: f64,
}

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let bf = format!("{root}/dataset/backfill");
    let cap: usize = envn("CAP", 1_000_000usize);
    let limit: u64 = envn("LIMIT", 0u64);
    let skew_d: i64 = envn("MAX_TS_SKEW_D", 3i64);

    // ── ціна WETH ──
    let eth = {
        let f = File::open(format!("{bf}/token-prices-hist.json")).expect("token-prices-hist.json");
        let v: Value = serde_json::from_reader(BufReader::new(f)).expect("hist json");
        let mut pts: Vec<(i64, f64)> = v["tokens"][wr_store::addrs::weth()]["prices"]
            .as_array()
            .expect("у token-prices-hist.json немає ряду WETH — без нього USD рахувати нема з чого")
            .iter()
            .filter_map(|p| Some((p[0].as_i64()?, p[1].as_f64()?)))
            .collect();
        pts.sort_by_key(|(t, _)| *t);
        eprintln!(
            "[dexswap] ціна WETH: {} точок, {}..{}",
            pts.len(),
            pts.first().map(|p| p.0).unwrap_or(0),
            pts.last().map(|p| p.0).unwrap_or(0)
        );
        EthPx { pts, skew: skew_d * 86400 }
    };

    // ── членство в групах ──
    let groups: HashMap<String, String> = {
        let f = File::open(format!("{bf}/group-membership.json")).expect("group-membership.json");
        let v: Value = serde_json::from_reader(BufReader::new(f)).expect("membership json");
        v["wallets"]
            .as_object()
            .map(|o| {
                o.iter()
                    .filter_map(|(k, w)| Some((k.clone(), w["group"].as_str()?.to_string())))
                    .collect()
            })
            .unwrap_or_default()
    };
    eprintln!("[dexswap] членство: {} гаманців", groups.len());

    // Один запит до знімка в пам'яті — база вже завантажена на старті.
    let snap = wr_store::snapshot();
    let st: Vec<(String, String)> = snap
        .tokens_tagged(wr_store::tags::DEXSWAP_STABLES)
        .into_iter()
        .map(|t| (t.contract.clone(), t.symbol_or_contract().to_string()))
        .collect();
    let ctx = Ctx {
        stables: st.iter().map(|(a, _)| a.clone()).collect(),
        sym_of: st.clone().into_iter().collect(),
        eth,
        groups,
        min_usd: envn("MIN_USD", 0.0f64),
    };

    let outdir = std::env::var("OUT_DIR").unwrap_or_else(|_| bf.clone());
    let out_path = format!("{outdir}/trades.jsonl");
    let tmp_path = format!("{out_path}.tmp");
    let mut out = std::io::BufWriter::new(File::create(&tmp_path).expect("trades.jsonl.tmp"));

    let mut buckets: HashMap<(String, String), Bucket> = HashMap::new();
    let mut order: VecDeque<(String, String)> = VecDeque::new();
    let mut st = Stats::default();
    let mut pos: HashMap<(String, String), Pos> = HashMap::new();

    let readers = csvin::readers(&format!("{bf}/transfers-grp.csv"));
    if readers.is_empty() {
        panic!("[dexswap] немає ні transfers-grp.csv, ні його архівів");
    }
    let mut rows: u64 = 0;
    'outer: for rd in readers {
        for line in rd.lines() {
            let line = match line {
                Ok(l) => l,
                Err(e) => panic!("[dexswap] читання обірвалось: {e}"),
            };
            if line.is_empty() || line.starts_with("wallet,") {
                continue;
            }
            rows += 1;
            if limit > 0 && rows > limit {
                break 'outer;
            }
            if rows % 20_000_000 == 0 {
                eprintln!("[dexswap] {rows} рядків, угод {}", st.trades);
            }
            let Some(r) = parse(&line) else {
                st.bad_row += 1;
                continue;
            };
            let (Some(ts), Ok(amt)) = (ts_unix(r.ts), r.amt.parse::<f64>()) else {
                st.bad_row += 1;
                continue;
            };
            if !amt.is_finite() || amt <= 0.0 {
                continue;
            }
            let is_in = r.dir == "i";
            let key = (r.w.to_string(), r.tx.to_string());
            let b = buckets.entry(key.clone()).or_insert_with(|| {
                order.push_back(key.clone());
                Bucket { ts, chain: r.chain.to_string(), legs: Vec::new(), seen: HashSet::new() }
            });
            // Дедуплікація: повторний краул (grp-refill) дописує ті самі рядки ще раз, а
            // подвоєна нога подвоїла б і нетто, і розмір угоди. Однакові ноги в межах
            // однієї транзакції відкидаються; це занижує рідкісний чесний випадок двох
            // рівних переказів — і саме такий бік помилки прийнятний: краще не порахувати,
            // ніж вигадати.
            if !b.seen.insert(hash_leg(r.tok, is_in, r.amt)) {
                st.dup_leg += 1;
                continue;
            }
            b.legs.push((r.tok.to_string(), r.sym.to_string(), is_in, amt));

            if buckets.len() > cap {
                for _ in 0..cap / 4 {
                    let Some(k) = order.pop_front() else { break };
                    if let Some(b) = buckets.remove(&k) {
                        emit(&k, b, &mut out, &ctx, &mut st, &mut pos);
                    }
                }
            }
        }
    }
    while let Some(k) = order.pop_front() {
        if let Some(b) = buckets.remove(&k) {
            emit(&k, b, &mut out, &ctx, &mut st, &mut pos);
        }
    }
    out.flush().expect("flush");
    drop(out);
    std::fs::rename(&tmp_path, &out_path).expect("rename trades.jsonl");

    // ── позиції: скільки від набраного вже злито ──
    let mut plist: Vec<Value> = pos
        .into_iter()
        .map(|((g, t), p)| {
            json!({
                "grp": g, "tok": t, "sym": p.sym,
                "buy_usd": r2(p.buy_usd), "sell_usd": r2(p.sell_usd),
                "net_usd": r2(p.buy_usd - p.sell_usd),
                "buy_units": p.buy_u, "sell_units": p.sell_u,
                "held_units": p.buy_u - p.sell_u,
                // Частка набраного, яку вже продали. Саме це число відповідає на «чи
                // виходять із активу», і саме воно НЕ виводиться з переказів між своїми.
                // Null, коли купівель у журналі немає: ділити нема на що, а 0 збрехав би
                // «ще нічого не продано».
                "sold_share": if p.buy_u > 0.0 { json!(r4(p.sell_u / p.buy_u)) } else { Value::Null },
                "trades": p.n, "first": p.first, "last": p.last,
            })
        })
        .collect();
    plist.sort_by(|a, b| {
        b["sell_usd"]
            .as_f64()
            .unwrap_or(0.0)
            .partial_cmp(&a["sell_usd"].as_f64().unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let summary = json!({
        "generated": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs(),
        "method": "ФАКТ: обидві ноги свопу в одній транзакції; сторона за НЕТТО в межах tx; USD з квотної ноги",
        "rows_read": rows,
        "trades": st.trades, "buys": st.buys, "sells": st.sells,
        "usd_bought": r2(st.usd_buy), "usd_sold": r2(st.usd_sell),
        "skipped": {
            "transit_zero_net": st.transit,
            "half_no_quote": st.half_no_quote,
            "not_a_swap_single_leg": st.single_leg,
            "quote_to_quote_fx": st.fx,
            "multi_base_basket": st.multi,
            "mixed_both_quote_sides": st.mixed,
            "no_eth_price": st.no_px,
            "below_min_usd": st.below_min,
            "dup_legs_removed": st.dup_leg,
            "bad_rows": st.bad_row,
        },
        "positions": plist,
    });
    let ppath = format!("{outdir}/trades-positions.json");
    let ptmp = format!("{ppath}.tmp");
    std::fs::write(&ptmp, serde_json::to_string_pretty(&summary).unwrap()).expect("write positions");
    std::fs::rename(&ptmp, &ppath).expect("rename positions");

    eprintln!("[dexswap] рядків {rows}; УГОД {} (купівель {}, продажів {})", st.trades, st.buys, st.sells);
    eprintln!("[dexswap] куплено ${:.0}, продано ${:.0}", st.usd_buy, st.usd_sell);
    eprintln!(
        "[dexswap] пропущено: транзит {} | без квотної ноги {} | одна нога {} | квота-квота {} | кошик {} | змішане {} | без ціни ETH {} | дублі ніг {} | биті рядки {}",
        st.transit, st.half_no_quote, st.single_leg, st.fx, st.multi, st.mixed, st.no_px, st.dup_leg, st.bad_row
    );
    eprintln!("[dexswap] -> {out_path} + {ppath}");
}

/// Одна транзакція → нуль або одна угода. Уся логіка сторони живе тут.
fn emit(
    key: &(String, String),
    b: Bucket,
    out: &mut impl Write,
    ctx: &Ctx,
    st: &mut Stats,
    pos: &mut HashMap<(String, String), Pos>,
) {
    if b.legs.len() < 2 {
        st.single_leg += 1;
        return;
    }

    // НЕТТО по токену: саме воно, а не окремі перекази, відрізняє угоду від транзиту.
    let mut net: HashMap<&str, f64> = HashMap::new();
    let mut gross: HashMap<&str, f64> = HashMap::new();
    let mut sym: HashMap<&str, &str> = HashMap::new();
    for (tok, s, is_in, amt) in &b.legs {
        *net.entry(tok.as_str()).or_insert(0.0) += if *is_in { *amt } else { -*amt };
        *gross.entry(tok.as_str()).or_insert(0.0) += *amt;
        sym.entry(tok.as_str()).or_insert(s.as_str());
    }
    // Поріг відносний: у токена з 18 знаками «нуль» після додавання f64 не точний нуль.
    let mut moved: Vec<(&str, f64)> = Vec::new();
    for (&tok, &n) in &net {
        let g = gross[tok];
        if n != 0.0 && n.abs() > 1e-9 * g.max(1e-18) {
            moved.push((tok, n));
        }
    }
    if moved.is_empty() {
        st.transit += 1;
        return;
    }

    let mut q_in = 0.0f64; // USD, що ЗАЙШЛИ гаманцю квотними активами
    let mut q_out = 0.0f64; // USD, що ВИЙШЛИ квотними активами
    let mut bases: Vec<(&str, f64)> = Vec::new();
    let mut px_missing = false;
    let mut quote_sym = "";
    for &(tok, n) in &moved {
        match classify(tok, &ctx.stables) {
            Cls::Base => bases.push((tok, n)),
            c => {
                let px = if c == Cls::Stable { Some(1.0) } else { ctx.eth.at(b.ts) };
                let Some(px) = px else {
                    px_missing = true;
                    continue;
                };
                if quote_sym.is_empty() {
                    quote_sym = if c == Cls::Stable {
                        ctx.sym_of.get(tok).map(String::as_str).unwrap_or("USD")
                    } else {
                        "WETH"
                    };
                }
                if n > 0.0 {
                    q_in += n * px
                } else {
                    q_out += -n * px
                }
            }
        }
    }
    if bases.is_empty() {
        st.fx += 1;
        return;
    }
    if q_in == 0.0 && q_out == 0.0 {
        // Це і є нативний своп: токенова нога тут, квотна — нативним ETH у сусідньому файлі.
        if px_missing {
            st.no_px += 1
        } else {
            st.half_no_quote += 1
        }
        return;
    }
    if q_in > 0.0 && q_out > 0.0 {
        st.mixed += 1;
        return;
    }
    if bases.len() > 1 {
        st.multi += 1;
        return;
    }

    let (tok, n) = bases[0];
    let (side, usd) = if n < 0.0 && q_in > 0.0 {
        ("SELL", q_in)
    } else if n > 0.0 && q_out > 0.0 {
        ("BUY", q_out)
    } else {
        st.mixed += 1;
        return;
    };
    if usd < ctx.min_usd {
        st.below_min += 1;
        return;
    }

    let amt = n.abs();
    let w = &key.0;
    let grp = ctx.groups.get(w).cloned().unwrap_or_else(|| "—".into());
    let symbol = sym.get(tok).copied().unwrap_or("?");
    let row = json!({
        "ts": b.ts, "w": w, "grp": grp, "side": side,
        "tok": tok, "sym": symbol, "amt": amt,
        "quote": quote_sym, "usd": r2(usd), "px": usd / amt,
        "venue": "DEX", "kind": "DEX_FACT", "chain": b.chain, "tx": key.1,
    });
    let _ = writeln!(out, "{row}");

    st.trades += 1;
    if side == "SELL" {
        st.sells += 1;
        st.usd_sell += usd
    } else {
        st.buys += 1;
        st.usd_buy += usd
    }
    let p = pos.entry((grp, tok.to_string())).or_default();
    if p.sym.is_empty() {
        p.sym = symbol.to_string()
    }
    if p.first == 0 || b.ts < p.first {
        p.first = b.ts
    }
    if b.ts > p.last {
        p.last = b.ts
    }
    p.n += 1;
    if side == "SELL" {
        p.sell_usd += usd;
        p.sell_u += amt
    } else {
        p.buy_usd += usd;
        p.buy_u += amt
    }
}
