//! Крок A: whale net-flow по токену з backfill.
//!
//! Наші ~500 гаманців зі списку backfill — це КИТИ, за якими стежимо. Кожен рядок
//! transfers.csv від їхньої точки зору: dir=i — кит НАБРАВ токен (accumulate),
//! dir=o — кит СКИНУВ (distribute), amount — уже нормалізований по decimals.
//!
//! Мета: по кожному (chain, token) порахувати, СКІЛЬКИ РІЗНИХ КИТІВ у ньому сидить
//! (net-positive holding) і скільки роздають — тобто «активи, де сидить великий
//! капітал». К-сть китів comparable між токенами (на відміну від net-units, бо
//! decimals/ціна різні), тож ранжуємо саме за нею.
//!
//! Спам-ейрдропи проявляться як «багато holders, ~0 distributors, дрібні суми» —
//! показуємо обидві колонки, щоб їх було видно й можна відсіяти далі.
//!
//! Парсинг: поля зліва wallet(0),chain(1),dir(2),ts(3),token(4),symbol(5..),
//! а counterparty/amount/tx_hash — останні 3 (без ком). Беремо amount = rsplit nth1,
//! token = split nth4, symbol збираємо як усе між token і останніми 3 полями.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Write};

struct Tok {
    symbol: String,
    net: HashMap<u32, f64>, // whale_id -> net (in - out) у token-одиницях
}

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let top_n: usize = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(40);
    // Необов'язковий 3-й арг: шлях, куди дампнути ПОВНИЙ список токенів-кандидатів
    // (distributors>=1) для Кроку B (фетч цін). TSV: chain,address,symbol,holders,distr,whales.
    let list_path = std::env::args().nth(3);

    let f = BufReader::new(
        File::open(format!("{root}/dataset/backfill/transfers.csv")).expect("transfers.csv"),
    );

    // Компактні id для гаманців (щоб не тримати адресу-рядок у кожному записі).
    let mut whale_id: HashMap<String, u32> = HashMap::new();
    let mut toks: HashMap<String, Tok> = HashMap::new();
    let mut rows = 0u64;

    for line in f.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };
        // зліва: wallet, chain, dir, ts, token
        let mut left = line.splitn(6, ',');
        let wallet = match left.next() {
            Some(w) => w,
            None => continue,
        };
        // Нульова адреса (mint/burn) — не кит; її величезна історія засмічувала б holders/net.
        if wallet == wr_store::addrs::zero() {
            continue;
        }
        let chain = left.next().unwrap_or("");
        let dir = left.next().unwrap_or("?");
        let _ts = left.next().unwrap_or("");
        let token = left.next().unwrap_or("");
        if dir != "i" && dir != "o" {
            continue; // заголовок / ?
        }
        // справа: tx_hash(0), amount(1), counterparty(2) — усі без ком
        let mut right = line.rsplit(',');
        let _hash = right.next();
        let amount: f64 = right.next().and_then(|a| a.parse().ok()).unwrap_or(0.0);
        // symbol нам для агрегації не потрібен порядково — візьмемо його з поля 5 (перше після token)
        let symbol = line.split(',').nth(5).unwrap_or("").to_string();

        if token.is_empty() || amount <= 0.0 {
            continue;
        }
        rows += 1;

        let next_id = whale_id.len() as u32;
        let wid = *whale_id.entry(wallet.to_string()).or_insert(next_id);

        let key = format!("{chain}\t{token}");
        let t = toks.entry(key).or_insert_with(|| Tok {
            symbol: symbol.clone(),
            net: HashMap::new(),
        });
        if t.symbol.is_empty() {
            t.symbol = symbol;
        }
        let e = t.net.entry(wid).or_insert(0.0);
        *e += if dir == "i" { amount } else { -amount };
    }

    // Агрегація по токену: holders (net>0), distributors (net<0), сума net.
    struct Agg {
        chain: String,
        token: String,
        symbol: String,
        holders: usize,
        distributors: usize,
        whales: usize,
        net_sum: f64,
    }
    const EPS: f64 = 1e-9;
    let mut aggs: Vec<Agg> = Vec::with_capacity(toks.len());
    for (key, t) in &toks {
        let (chain, token) = key.split_once('\t').unwrap_or(("", ""));
        let mut holders = 0usize;
        let mut distributors = 0usize;
        let mut net_sum = 0.0;
        for &n in t.net.values() {
            if n > EPS {
                holders += 1;
            } else if n < -EPS {
                distributors += 1;
            }
            net_sum += n;
        }
        aggs.push(Agg {
            chain: chain.to_string(),
            token: token.to_string(),
            symbol: t.symbol.clone(),
            holders,
            distributors,
            whales: t.net.len(),
            net_sum,
        });
    }

    println!("[netflow] рядків оброблено: {rows}, унікальних китів: {}, токенів: {}", whale_id.len(), toks.len());

    // Спам-ейрдропи: багато holders, але НІХТО не відправляв (distributors==0) — токен
    // нічого не вартий, тож кити його ігнорують. Реальний актив кити й набирають, і скидають.
    let spam = aggs.iter().filter(|a| a.distributors == 0).count();
    let traded: Vec<&Agg> = {
        let mut v: Vec<&Agg> = aggs.iter().filter(|a| a.distributors >= 1 && a.holders >= 1).collect();
        v.sort_by(|a, b| {
            let ka = a.holders + a.distributors;
            let kb = b.holders + b.distributors;
            kb.cmp(&ka).then(b.distributors.cmp(&a.distributors))
        });
        v
    };
    println!("спам-кандидатів (distributors==0): {spam} з {} токенів", toks.len());

    println!("\n=== ТОП-{top_n} активів з РЕАЛЬНИМ 2-стороннім рухом китів ===");
    println!("(hold=китів набрали net+, distr=скинули net-, all=торкнулись)");
    println!("{:>4} {:>4} {:>4}  {:<16} {:<9} {}", "hold", "distr", "all", "symbol", "chain", "token");
    for a in traded.iter().take(top_n) {
        let sym: String = a.symbol.chars().take(16).collect();
        let short: String = a.token.chars().take(14).collect();
        println!(
            "{:>4} {:>4} {:>4}  {:<16} {:<9} {}…",
            a.holders, a.distributors, a.whales, sym, a.chain, short
        );
    }
    println!("\nактивів із 2-стороннім рухом (distributors>=1): {}", traded.len());

    // Дамп повного списку кандидатів для Кроку B.
    if let Some(path) = list_path {
        match File::create(&path) {
            Ok(mut w) => {
                let _ = writeln!(w, "chain\taddress\tsymbol\tholders\tdistributors\twhales");
                let mut n = 0;
                for a in &traded {
                    // symbol очищаємо від табів/новорядків, щоб TSV не поламався
                    let sym: String = a.symbol.chars().filter(|c| *c != '\t' && *c != '\n' && *c != '\r').collect();
                    if writeln!(w, "{}\t{}\t{}\t{}\t{}\t{}", a.chain, a.token, sym, a.holders, a.distributors, a.whales).is_ok() {
                        n += 1;
                    }
                }
                println!("список кандидатів записано: {path} ({n} токенів)");
            }
            Err(e) => eprintln!("[netflow] не вдалось записати список {path}: {e}"),
        }
    }
}
