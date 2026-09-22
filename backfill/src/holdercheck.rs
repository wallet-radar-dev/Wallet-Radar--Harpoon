//! holdercheck — хто саме стоїть за великою часткою емісії, і чи це взагалі учасник групи.
//!
//! Навіщо: підсумок «PEPE 28.6% емісії» нічого не вартий, поки не видно холдерів. Досі
//! кожна така перевірка робилась вручну й наново, бо realbal зберігав лише агрегати.
//! Тепер він пише пари на диск, і цей бін читає їх разом із накопичувальною базою
//! патернів — жодного повторного запиту до Etherscan.
//!
//! Ловить три способи, якими частка емісії роздувається:
//!   1. холдер із міткою біржі — це клієнтські кошти, а не позиція групи;
//!   2. контракт протоколу (sUSDS/wstETH/aEth*) — сховище, а не гаманець;
//!   3. одна адреса на майже всю емісію — зазвичай емітент або казна проєкту.
//!
//! Вихід: dataset/backfill/holdercheck.json + запис патернів у базу.
//! Args: ROOT. Env: TOP(25) MIN_USD(1e6) SOLO_PCT(50)
use backfill::known::{self, pat};
use backfill::patterns::{Finding, PatternDb};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Write};

fn envn<T: std::str::FromStr>(k: &str, d: T) -> T {
    std::env::var(k).ok().and_then(|s| s.parse().ok()).unwrap_or(d)
}

struct Pos {
    w: String,
    real: f64,
    usd: f64,
}

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let bf = format!("{root}/dataset/backfill");
    let top: usize = envn("TOP", 25);
    let min_usd: f64 = envn("MIN_USD", 1e6);
    let solo_pct: f64 = envn("SOLO_PCT", 50.0);

    let mut db = PatternDb::load(&bf);
    eprintln!("[holdercheck] база патернів: {} адрес", db.map.len());

    // ціни й емісія — щоб рахувати частку в тих самих одиницях, що realbal
    let mut price: HashMap<String, f64> = HashMap::new();
    if let Ok(f) = File::open(format!("{bf}/token-prices.json")) {
        if let Ok(v) = serde_json::from_reader::<_, Value>(BufReader::new(f)) {
            if let Some(o) = v["tokens"].as_object() {
                for (c, x) in o {
                    price.insert(c.to_lowercase(), x["usd"].as_f64().unwrap_or(0.0));
                }
            }
        }
    }
    // Емісія має два джерела, і друге — не забаганка: у token-supply.json її для частини
    // токенів просто немає, і без запасного tokenpnl.jsonl частка рахувалась від нуля.
    // Перший прогін дав через це «1166508053258547968% емісії» — ділення на 1e-9.
    let mut supply: HashMap<String, f64> = HashMap::new();
    if let Ok(f) = File::open(format!("{bf}/token-supply.json")) {
        if let Ok(m) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(f)) {
            for (c, v) in m {
                if let Some(s) = v["supply"].as_f64() {
                    if s > 0.0 {
                        supply.insert(c.to_lowercase(), s);
                    }
                }
            }
        }
    }
    if let Ok(f) = File::open(format!("{bf}/tokenpnl.jsonl")) {
        for line in BufReader::new(f).lines().map_while(Result::ok) {
            if let Ok(d) = serde_json::from_str::<Value>(&line) {
                let c = d["contract"].as_str().unwrap_or("").to_lowercase();
                if let Some(s) = d["supply"].as_f64() {
                    if s > 0.0 {
                        supply.entry(c).or_insert(s);
                    }
                }
            }
        }
    }
    eprintln!("[holdercheck] відома емісія для {} токенів", supply.len());

    // пари з realbal — реальні, підтверджені баланси
    let mut by_tok: HashMap<String, (String, Vec<Pos>)> = HashMap::new();
    let f = File::open(format!("{bf}/net6k-real-pairs.jsonl"))
        .expect("нема net6k-real-pairs.jsonl — спершу realbal");
    for line in BufReader::with_capacity(1 << 20, f).lines().map_while(Result::ok) {
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        let real = v["real"].as_f64().unwrap_or(0.0);
        if real <= 0.0 {
            continue;
        }
        let tok = v["tok"].as_str().unwrap_or("").to_lowercase();
        let sym = v["sym"].as_str().unwrap_or("").to_string();
        let usd = real * price.get(&tok).copied().unwrap_or(0.0);
        by_tok
            .entry(tok)
            .or_insert((sym, Vec::new()))
            .1
            .push(Pos { w: v["w"].as_str().unwrap_or("").to_lowercase(), real, usd });
    }
    eprintln!("[holdercheck] токенів із підтвердженими балансами: {}", by_tok.len());

    // найбільші позиції за доларом
    let mut toks: Vec<(&String, &(String, Vec<Pos>))> = by_tok.iter().collect();
    toks.sort_by(|a, b| {
        let s = |x: &(String, Vec<Pos>)| x.1.iter().map(|p| p.usd).sum::<f64>();
        s(b.1).partial_cmp(&s(a.1)).unwrap()
    });

    let mut out: Vec<Value> = Vec::new();
    let mut flagged = 0usize;

    for (tok, (sym, poss)) in toks.into_iter().take(top) {
        let tot_usd: f64 = poss.iter().map(|p| p.usd).sum();
        if tot_usd < min_usd {
            continue;
        }
        let tot_held: f64 = poss.iter().map(|p| p.real).sum();
        let sup = supply.get(tok).copied().unwrap_or(0.0);
        // Фаза 0.2: частка емісії у % — зі СЛОВНИКА. emission_pct(b,s) ≡ старий
        // `if s>0 {100*b/s} else {0}` бітово (той самий порядок операцій) → байт-паритет.
        let em = wr_dict::emission_pct(tot_held, sup);

        let mut rows: Vec<&Pos> = poss.iter().collect();
        rows.sort_by(|a, b| b.usd.partial_cmp(&a.usd).unwrap());

        // скільки долара сидить на мічених біржах / контрактах — це не позиція групи
        let mut cex_usd = 0.0;
        let mut contract_usd = 0.0;
        for p in &rows {
            if db.has(&p.w, pat::CEX_LABELED) {
                cex_usd += p.usd;
            } else if db.has(&p.w, pat::CONTRACT) {
                contract_usd += p.usd;
            }
        }
        let clean_usd = tot_usd - cex_usd - contract_usd;

        // чому частка може бути фікцією
        let mut why: Vec<&str> = Vec::new();
        if known::is_protocol_token(sym) {
            why.push("токен-обгортка протоколу");
        }
        let solo = rows.first().map(|p| 100.0 * p.usd / tot_usd.max(1e-9)).unwrap_or(0.0);
        if solo >= solo_pct {
            why.push("одна адреса тримає більшість");
        }
        if cex_usd > 0.3 * tot_usd {
            why.push("переважно біржові адреси");
        }
        if !why.is_empty() {
            flagged += 1;
        }

        println!("\n=== {sym} · ${:.0} · {:.2}% емісії · холдерів {} ===", tot_usd, em, rows.len());
        if !why.is_empty() {
            println!("  ⚠ {}", why.join("; "));
        }
        if cex_usd > 0.0 || contract_usd > 0.0 {
            println!(
                "  біржі ${:.0} · контракти ${:.0} · ЧИСТО ${:.0}",
                cex_usd, contract_usd, clean_usd
            );
        }
        for p in rows.iter().take(6) {
            let mut tags: Vec<&str> = db.pats(&p.w);
            tags.retain(|t| !t.starts_with("GRP-") && *t != pat::HOLDER);
            tags.sort_unstable();
            // Без відомої емісії частку не рахуємо взагалі. Ділення на «майже нуль»
            // тихо давало числа на 10^18 і виглядало як знахідка, а не як відсутність даних.
            let share = if sup > 0.0 { Some(wr_dict::emission_pct(p.real, sup)) } else { None };
            println!(
                "  {} ${:>13.0} {:>10} емісії  {}",
                &p.w[..20.min(p.w.len())],
                p.usd,
                share.map(|x| format!("{x:.2}%")).unwrap_or_else(|| "н/д".into()),
                tags.join(" ")
            );
            // адреса, яка сама тримає помітну частку емісії, — окремий патерн:
            // саме такі й треба перевіряти руками, а не тисячі дрібних
            if share.unwrap_or(0.0) >= 1.0
                && !db.has(&p.w, pat::CEX_LABELED)
                && !db.has(&p.w, pat::CONTRACT)
            {
                db.add(
                    &p.w.clone(),
                    Finding::new("WHALE-POSITION", "holdercheck")
                        .evk("em", sym, share.unwrap_or(0.0))
                        .evk("usd", sym, p.usd),
                );
            }
        }

        out.push(json!({
            "token": tok, "symbol": sym, "totalUsd": tot_usd, "emissionPct": em,
            "holders": rows.len(), "cexUsd": cex_usd, "contractUsd": contract_usd,
            "cleanUsd": clean_usd, "soloPct": solo, "warnings": why,
            "top": rows.iter().take(10).map(|p| json!({
                "addr": p.w, "usd": p.usd,
                "emissionPct": if sup > 0.0 { Some(wr_dict::emission_pct(p.real, sup)) } else { None },
                "pats": db.pats(&p.w)})).collect::<Vec<_>>()
        }));
    }

    // Дозаписуємо позиції по-токенно, беручи їх із файлу пар, а не з Etherscan: realbal
    // писав докази під спільним ключем, і в кита з 42 активами лишався видимим один.
    // Перезапитувати 6000 балансів заради цього — марна трата квоти, дані вже на диску.
    {
        let mut n = 0usize;
        for (tok, (sym, poss)) in &by_tok {
            let p_usd = price.get(tok).copied().unwrap_or(0.0);
            if p_usd <= 0.0 || sym.is_empty() {
                continue;
            }
            for p in poss {
                if p.usd < 10_000.0 {
                    continue;
                }
                db.add(&p.w, Finding::new(pat::HOLDER, "realbal").evk("usd", sym, p.usd));
                n += 1;
            }
        }
        eprintln!("[holdercheck] докази позицій по-токенно: {n}");
    }

    // ЯДРО. Досі група описувалась через токени — «PEPE 29% емісії». Але позицію тримають
    // адреси, і та сама адреса з'являється в різних токенах: саме повторення робить її
    // ядром, а не розмір однієї позиції. Тут рахуємо по всіх підтверджених балансах, а не
    // лише по топу, — інакше кит із десятьма середніми позиціями лишиться невидимим.
    {
        let mut per_addr: HashMap<&str, (f64, Vec<&str>)> = HashMap::new();
        for (tok, (sym, poss)) in &by_tok {
            let p_usd = price.get(tok).copied().unwrap_or(0.0);
            if p_usd <= 0.0 {
                continue;
            }
            for p in poss {
                if p.usd < 100_000.0 {
                    continue;
                }
                let e = per_addr.entry(p.w.as_str()).or_insert((0.0, Vec::new()));
                e.0 += p.usd;
                e.1.push(sym.as_str());
            }
        }
        let mut core: Vec<(&str, f64, Vec<&str>)> =
            per_addr.into_iter().map(|(a, (u, s))| (a, u, s)).collect();
        core.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

        println!("\n=== ЯДРО: найбільші власники по ВСІХ токенах (позиції ≥$100k) ===");
        println!("{:<44} {:>14} {:>5}  {}", "АДРЕСА", "USD", "ТОК", "ПАТЕРНИ / АКТИВИ");
        for (a, usd, syms) in core.iter().take(20) {
            let mut tags: Vec<&str> = db.pats(a);
            tags.retain(|t| !t.starts_with("GRP-") && *t != pat::HOLDER);
            tags.sort_unstable();
            let mut s = syms.clone();
            s.sort_unstable();
            s.dedup();
            let shown: Vec<&&str> = s.iter().take(6).collect();
            println!(
                "{a} {usd:>14.0} {:>5}  {} | {:?}",
                s.len(),
                tags.join(" "),
                shown
            );
            // кит у ≥5 різних активах — це вже не інвестор в одну ідею, а розподілений
            // портфель; такі адреси й формують кістяк групи
            if s.len() >= 5 && *usd >= 5e6 {
                db.add(
                    a,
                    Finding::new("MULTI-ASSET-WHALE", "holdercheck")
                        .ev("usd", *usd)
                        .ev("tokens", s.len() as f64),
                );
            }
        }
        let core_usd: f64 = core.iter().take(20).map(|x| x.1).sum();
        let all_usd: f64 = core.iter().map(|x| x.1).sum();
        println!(
            "\nтоп-20 адрес тримають ${:.0} із ${:.0} ({:.1}%) — власників усього {}",
            core_usd,
            all_usd,
            100.0 * core_usd / all_usd.max(1e-9),
            core.len()
        );
    }

    let clean_total: f64 = out.iter().filter_map(|r| r["cleanUsd"].as_f64()).sum();
    let raw_total: f64 = out.iter().filter_map(|r| r["totalUsd"].as_f64()).sum();
    println!("\n=== ПІДСУМОК ПО ТОП-{} ===", out.len());
    println!("сирий підсумок ${:.0} · без бірж і контрактів ${:.0}", raw_total, clean_total);
    println!("позицій із застереженням: {flagged} із {}", out.len());

    let _ = File::create(format!("{bf}/holdercheck.json"))
        .unwrap()
        .write_all(serde_json::to_string(&json!({"rawUsd": raw_total, "cleanUsd": clean_total,
            "rows": out})).unwrap().as_bytes());
    db.save(&bf);
}

#[cfg(test)]
mod parity_tests {
    /// БАЙТ-ПАРИТЕТ (Фаза 0.2): старий вираз `100.0*b/s` і `wr_dict::emission_pct(b,s)`
    /// дають БІТОВО той самий f64 для всіх s>0 (той самий порядок операцій), і 0.0 для s<=0
    /// (як старе `if s>0 {..} else {0.0}`). Перевіряємо to_bits().
    #[test]
    fn emission_pct_bit_identical_to_old_expr() {
        let sups: [f64; 6] = [1.0, 1.5, 1e6, 528_786_477.77, 1e18, 183_987_824_878.0];
        let bals: [f64; 6] = [0.0, 1.0, 250_000.0, 1e9, 9.9e17, 1.23456789e12];
        for &s in &sups {
            for &b in &bals {
                let old = 100.0 * b / s; // порядок як у holdercheck (s>0 гілка)
                assert_eq!(old.to_bits(), wr_dict::emission_pct(b, s).to_bits(),
                    "розбіжність b={b}, s={s}");
            }
        }
        // гілка s<=0 -> 0.0 (як старий else)
        assert_eq!(wr_dict::emission_pct(123.0, 0.0), 0.0);
        assert_eq!(wr_dict::emission_pct(123.0, -5.0), 0.0);
    }
}
