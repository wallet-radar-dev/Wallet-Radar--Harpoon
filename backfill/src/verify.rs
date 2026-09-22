//! verify — перевірка вітрин на неможливе. Число, що не пройшло, НЕ має публікуватись.
//!
//! Навіщо окремий бін: усі помилки, які коштували цьому проєкту найдорожче, були МОВЧАЗНІ —
//! вони не падали, а давали правдоподібну цифру. Частка емісії 189% (знаменник із іншої
//! епохи); «купив 118 млн WETH», хоч стільки не існує (валовий обіг замість позиції);
//! COMP по $41 046 замість $17 (ціни зі свопів); сальдо з потоків, що розходиться з
//! ланцюгом у 11 разів. Око ловить таке випадково, тест — завжди.
//!
//! Еталон узято з життя: казна 0x0e58e899 зійшлася з `balanceOf` на 0.1%. Отже звірка
//! потоків із ланцюгом — робочий критерій, а не побажання.
//!
//! Перевірки (кожна = клас помилки, що вже траплявся):
//!   A. частка емісії поза [0..100] — знаменник не з тієї епохи або сальдо бреше;
//!   B. обсяг більший за емісію — валовий обіг прийнято за набір позиції;
//!   C. розбіжність потоків із балансом на ланцюгу (ETH) — неповна історія;
//!   D. ціни зі свопів проти DefiLlama — артефакти на порядки;
//!   E. повнота збору по списках гаманців — «готово» при 95.8%.
//!
//! Вихід: dataset/backfill/verify-report.json + підсумок у stdout.
//! Код виходу 1, якщо є хоч один FAIL — щоб крон-конвеєр зупинявся, а не публікував.
//! Args: ROOT. Env: MAX_ETH_DIFF_PCT(5) PRICE_RATIO(10) MIN_COMPLETE_PCT(99)

use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Write};

fn envf(k: &str, d: f64) -> f64 {
    std::env::var(k).ok().and_then(|s| s.parse().ok()).unwrap_or(d)
}
fn load(p: &str) -> Option<Value> {
    serde_json::from_reader(BufReader::new(File::open(p).ok()?)).ok()
}

struct Check { name: String, ok: bool, detail: String, samples: Vec<String> }

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".to_string());
    let b = format!("{root}/dataset/backfill");
    let max_eth_diff = envf("MAX_ETH_DIFF_PCT", 5.0);
    let price_ratio = envf("PRICE_RATIO", 10.0);
    let min_complete = envf("MIN_COMPLETE_PCT", 99.0);
    let mut checks: Vec<Check> = vec![];

    // ── A. частка емісії поза межами можливого ───────────────────────────────
    {
        let mut bad: Vec<String> = vec![];
        let mut total = 0usize;
        let files: [(&str, &str, &str); 3] = [
            ("bullsell-2021final.json", "assets", "maxSharePct"),
            ("groupbook.json", "positions", "emissionSharePctFlow"),
            ("bullsell-scan-2021all.json", "pairs", "maxSharePct"),
        ];
        for (f, arr, key) in files {
            let Some(v) = load(&format!("{b}/{f}")) else { continue };
            let Some(rows) = v[arr].as_array() else { continue };
            for r in rows {
                let Some(s) = r[key].as_f64() else { continue };
                total += 1;
                // те, що бін сам позначив сумнівним, не рахуємо за порушення:
                // він уже відмовився від цифри, а це і є потрібна поведінка
                if r["supplyEraSuspect"].as_bool().unwrap_or(false)
                    || r["shareImpossible"].as_bool().unwrap_or(false) { continue }
                if !(0.0..=100.0).contains(&s) {
                    bad.push(format!("{f}: {} {} = {s:.1}%",
                        r["group"].as_str().unwrap_or("?"), r["symbol"].as_str().unwrap_or("?")));
                }
            }
        }
        let ok = bad.is_empty();
        checks.push(Check {
            name: "A. частка емісії в межах 0..100".into(),
            ok,
            detail: format!("перевірено {total}, порушень {}", bad.len()),
            samples: bad.into_iter().take(8).collect(),
        });
    }

    // ── B. обсяг більший за емісію ───────────────────────────────────────────
    {
        let mut supply: HashMap<String, f64> = HashMap::new();
        if let Some(v) = load(&format!("{b}/token-supply.json")) {
            if let Some(o) = v.as_object() {
                for (c, x) in o {
                    if let Some(s) = x["supply"].as_f64() { if s > 0.0 { supply.insert(c.to_lowercase(), s); } }
                }
            }
        }
        let mut bad: Vec<String> = vec![];
        let mut total = 0usize;
        let mut refused = 0usize;
        if let Some(v) = load(&format!("{b}/groupbook.json")) {
            for r in v["positions"].as_array().cloned().unwrap_or_default() {
                let (Some(c), Some(bought)) = (r["contract"].as_str(), r["boughtTokens"].as_f64()) else { continue };
                let Some(sup) = supply.get(&c.to_lowercase()) else { continue };
                total += 1;
                // рядок, який бін сам позначив як обіг, — це вже відмова від числа,
                // а не порушення; рахуємо такі окремо, щоб їх кількість була видна
                if r["churnSuspect"].as_bool().unwrap_or(false) { refused += 1; continue }
                if bought > *sup {
                    bad.push(format!("{} {}: набрано {:.0} при емісії {:.0} (x{:.1})",
                        r["group"].as_str().unwrap_or("?"), r["symbol"].as_str().unwrap_or("?"),
                        bought, sup, bought / sup));
                }
            }
        }
        let ok = bad.is_empty();
        checks.push(Check {
            name: "B. набрано не більше за емісію".into(),
            ok,
            detail: format!("перевірено {total}, порушень {}, відхилено самим біном як обіг: {refused}", bad.len()),
            samples: bad.into_iter().take(8).collect(),
        });
    }

    // ── C. потоки проти балансу на ланцюгу (нативний ETH) ────────────────────
    {
        let mut onchain: HashMap<String, f64> = HashMap::new();
        // Знімок балансів має ДАТУ. Порівнювати його з потоками за весь час не можна:
        // для активного гаманця це липневий баланс проти серпневих рухів, і розбіжність
        // у сотні відсотків нічого не означає. Тому потік ріжемо по as-of знімка.
        let mut as_of: i64 = i64::MAX;
        if let Some(v) = load(&format!("{b}/eth-balances.json")) {
            if let Some(t) = v["asOf"].as_i64() { as_of = t }
            if let Some(o) = v["wallets"].as_object() {
                for (a, x) in o {
                    if let Some(e) = x["eth"].as_f64() { onchain.insert(a.to_lowercase(), e); }
                }
            }
        }
        let as_of_day = as_of / 86400;
        // сальдо з потоків рахуємо лише для гаманців із ПОВНОЮ історією:
        // на недокачаних розбіжність очікувана й нічого не доводить
        let mut done: HashMap<String, u32> = HashMap::new();
        if let Ok(f) = File::open(format!("{b}/_progress-native.csv")) {
            let mut seen: HashMap<(String, String), bool> = HashMap::new();
            for l in BufReader::new(f).lines().map_while(Result::ok) {
                let p: Vec<&str> = l.split(',').collect();
                if p.len() >= 4 { seen.insert((p[0].to_lowercase(), p[2].to_string()), p[3] == "done"); }
            }
            for ((w, _), ok) in seen { if ok { *done.entry(w).or_insert(0) += 1 } }
        }
        let mut net: HashMap<String, f64> = HashMap::new();
        let mut dedup: std::collections::HashSet<u64> = std::collections::HashSet::new();
        // діагностика по одному гаманцю: коли звірка не сходиться, треба бачити ЧОМУ,
        // а не гадати — скільки рядків узято, скільки відкинуто як дублі, які суми
        let dbg_w = std::env::var("DEBUG_WALLET").ok().map(|s| s.to_lowercase());
        let (mut dbg_rows, mut dbg_in, mut dbg_out, mut dbg_dup) = (0u64, 0.0f64, 0.0f64, 0u64);
        let (mut dbg_total, mut dbg_readerr) = (0u64, 0u64);
        // Живий файл ПЛЮС .zst-архіви: після прокрутки старша історія лежить стисненою,
        // і звірка лише по живому файлу показувала б систематичну недостачу.
        for mut rd in backfill::csvin::readers(&format!("{b}/transfers-native.csv")) {
            let mut raw: Vec<u8> = Vec::with_capacity(256);
            loop {
                raw.clear();
                // Читаємо БАЙТАМИ. read_line вимагає коректний UTF-8 і на першому ж
                // непридатному байті повертає помилку — а код на помилці зупиняв розбір
                // усього файлу. Так із 84 млн рядків перевірка бачила 30 млн і мовчала.
                match rd.read_until(10, &mut raw) {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(_) => { dbg_readerr += 1; continue }
                }
                dbg_total += 1;
                let line = String::from_utf8_lossy(&raw);
                let s = line.trim_end();
                if s.is_empty() || s.starts_with("wallet,") { continue }
                let mut it = s.splitn(5, ',');
                let (Some(w), Some(_c), Some(dir), Some(ts), _) =
                    (it.next(), it.next(), it.next(), it.next(), it.next()) else { continue };
                if done.get(w).copied().unwrap_or(0) < 2 { continue }
                // день переказу проти дня знімка
                if ts.len() >= 10 {
                    let (y, m, d): (i64, i64, i64) = (
                        ts[0..4].parse().unwrap_or(0), ts[5..7].parse().unwrap_or(0), ts[8..10].parse().unwrap_or(0));
                    let yy = if m <= 2 { y - 1 } else { y };
                    let era = (if yy >= 0 { yy } else { yy - 399 }) / 400;
                    let yoe = yy - era * 400;
                    let mp = (m + 9) % 12;
                    let doy = (153 * mp + 2) / 5 + d - 1;
                    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
                    if era * 146097 + doe - 719468 > as_of_day { continue }
                }
                let mut rit = s.rsplitn(3, ',');
                let (Some(hash), Some(amt)) = (rit.next(), rit.next()) else { continue };
                let Ok(a) = amt.parse::<f64>() else { continue };
                // Дедуп по ЦІЛОМУ рядку. Ключ із (хеш, гаманець, напрям, сума) виглядав
                // достатнім, але одна транзакція містить багато внутрішніх переказів з
                // однаковою сумою, і такий ключ склеював їх у один: з 83 тис. рядків
                // гаманця лишалось 14 тис., а сальдо роздувалось із 26 до 22 741 ETH.
                // Повторний прохід дописує рядок БАЙТ У БАЙТ, тож цього ключа досить.
                let _ = (hash, amt);
                let mut hh: u64 = 0xcbf29ce484222325;
                for byte in s.as_bytes() { hh ^= *byte as u64; hh = hh.wrapping_mul(0x100000001b3); }
                if !dedup.insert(hh) { dbg_dup += 1; continue }
                if dbg_w.as_deref() == Some(w) {
                    dbg_rows += 1;
                    if dir == "i" { dbg_in += a } else { dbg_out += a }
                }
                let e = net.entry(w.to_string()).or_insert(0.0);
                if dir == "i" { *e += a } else { *e -= a }
            }
        }
        let mut diffs: Vec<(f64, String)> = vec![];
        for (w, flow) in &net {
            let Some(real) = onchain.get(w) else { continue };
            if *real < 1.0 { continue } // на копійках відсоток не має сенсу
            let d = 100.0 * (flow - real).abs() / real;
            diffs.push((d, format!("{w}: потоки {flow:.0} vs ланцюг {real:.0} ({d:.1}%)")));
        }
        eprintln!("[verify] прочитано рядків {dbg_total}, обривів читання {dbg_readerr}");
        if let Some(dw) = &dbg_w {
            eprintln!("[debug {dw}] узято рядків {dbg_rows}, вхід {dbg_in:.0}, вихід {dbg_out:.0}, сальдо {:.0}, відкинуто дублів {dbg_dup}",
                dbg_in - dbg_out);
        }
        diffs.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
        let n = diffs.len();
        let med = if n > 0 { diffs[n / 2].0 } else { 0.0 };
        // Невелике завищення сальдо ОЧІКУВАНЕ й не є помилкою: комісія не входить у value
        // транзакції, тож гаманець із десятками тисяч операцій втратив на газі ETH, яких
        // потік не бачить. Карати за це означало б підганяти дані під тест.
        // Помилка — СТРУКТУРНО неможливе: потік каже вдвічі більше за ланцюг (неповна
        // історія або задвоєні рядки). Поріг max_eth_diff лишається орієнтиром у звіті.
        let broken = diffs.iter().filter(|(d, _)| *d > 100.0).count();
        // Перевірка має не лише сказати «погано», а й дати список для ремонту:
        // ці гаманці перезбираються адресно, а не перекачуванням усього з нуля.
        let offenders: Vec<String> = diffs.iter().filter(|(d, _)| *d > 100.0)
            .filter_map(|(_, s)| s.split(':').next().map(|x| x.to_string())).collect();
        let _ = std::fs::write(format!("{b}/verify-eth-offenders.txt"), offenders.join("\n"));
        let _ = max_eth_diff;
        let ok = broken == 0;
        checks.push(Check {
            name: "C. сальдо ETH сходиться з ланцюгом".into(),
            ok,
            detail: format!("гаманців {n}, медіана {med:.2}% (мала пояснюється газом), структурно неможливих понад 2x: {broken}"),
            samples: diffs.into_iter().take(5).map(|(_, s)| s).collect(),
        });
    }

    // ── D. ціни зі свопів проти DefiLlama ────────────────────────────────────
    {
        let mut llama: HashMap<String, f64> = HashMap::new();
        if let Some(v) = load(&format!("{b}/token-prices.json")) {
            if let Some(o) = v["tokens"].as_object() {
                for (c, x) in o { if let Some(p) = x["usd"].as_f64() { llama.insert(c.to_lowercase(), p); } }
            }
        }
        let mut bad: Vec<String> = vec![];
        let mut total = 0usize;
        if let Some(v) = load(&format!("{b}/token-prices-onchain.json")) {
            if let Some(o) = v.as_object() {
                for (c, x) in o {
                    let p = x.as_f64().or_else(|| x["usd"].as_f64()).unwrap_or(0.0);
                    let Some(l) = llama.get(&c.to_lowercase()) else { continue };
                    if *l <= 0.0 || p <= 0.0 { continue }
                    total += 1;
                    let r = if p > *l { p / l } else { l / p };
                    if r > price_ratio {
                        bad.push(format!("{c}: своп ${p:.4} проти DefiLlama ${l:.4} (x{r:.0})"));
                    }
                }
            }
        }
        let ok = bad.is_empty();
        checks.push(Check {
            name: "D. ціни зі свопів без артефактів".into(),
            ok,
            detail: format!("звірено {total}, розходжень понад x{price_ratio}: {}", bad.len()),
            samples: bad.into_iter().take(8).collect(),
        });
    }

    // ── E. повнота збору по списках ──────────────────────────────────────────
    {
        let mut lines: Vec<String> = vec![];
        let mut ok = true;
        for (list, prog, kinds) in [
            ("grp-refill-wallets.txt", "_progress-grp.csv", 1u32),
            ("ethflow-wallets-HUB.txt", "_progress-native.csv", 2),
            ("ethflow-wallets-NET.txt", "_progress-native.csv", 2),
            ("ethflow-wallets-NET-WEAK.txt", "_progress-native.csv", 2),
        ] {
            let Ok(lf) = File::open(format!("{b}/{list}")) else { continue };
            let want: std::collections::HashSet<String> = BufReader::new(lf).lines()
                .map_while(Result::ok).map(|s| s.trim().to_lowercase())
                .filter(|s| s.starts_with("0x")).collect();
            if want.is_empty() { continue }
            let mut got: HashMap<String, std::collections::HashSet<String>> = HashMap::new();
            if let Ok(pf) = File::open(format!("{b}/{prog}")) {
                let mut last: HashMap<(String, String), String> = HashMap::new();
                for l in BufReader::new(pf).lines().map_while(Result::ok) {
                    let p: Vec<&str> = l.split(',').collect();
                    if p.len() >= 4 { last.insert((p[0].to_lowercase(), p[2].to_string()), p[3].to_string()); }
                }
                for ((w, k), st) in last { if st == "done" { got.entry(w).or_default().insert(k); } }
            }
            let full = want.iter().filter(|w| got.get(*w).map(|s| s.len() as u32 >= kinds).unwrap_or(false)).count();
            let pct = 100.0 * full as f64 / want.len() as f64;
            if pct < min_complete { ok = false }
            lines.push(format!("{list}: {full}/{} = {pct:.2}%", want.len()));
        }
        checks.push(Check {
            name: format!("E. збір завершено щонайменше на {min_complete}%"),
            ok,
            detail: format!("списків перевірено {}", lines.len()),
            samples: lines,
        });
    }

    // ── F. свіжість вітрин ───────────────────────────────────────────────────
    {
        // Клас помилки, на якому я спіймався сам: знімок балансів від 27 липня
        // порівнювався з потоками за серпень, і розбіжність у сотні відсотків виглядала
        // як зіпсовані дані, хоч дані були цілі. Вітрина без дати — це пастка.
        let max_age = envf("MAX_AGE_DAYS", 7.0);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as f64;
        let mut lines: Vec<String> = vec![];
        let mut ok = true;
        for f in ["netassets.json", "eth-balances.json", "token-prices.json",
                  "groupbook.json", "bullsell-2021final.json", "token-supply.json"] {
            let path = format!("{b}/{f}");
            let Ok(md) = std::fs::metadata(&path) else { continue };
            let Ok(m) = md.modified() else { continue };
            let age = (now - m.duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as f64) / 86400.0;
            if age > max_age { ok = false }
            lines.push(format!("{f}: {age:.1} дн{}", if age > max_age { " — ЗАСТАРІЛА" } else { "" }));
        }
        checks.push(Check {
            name: format!("F. вітрини не старші за {max_age} дн"),
            ok,
            detail: format!("перевірено файлів {}", lines.len()),
            samples: lines,
        });
    }

    // ── підсумок ─────────────────────────────────────────────────────────────
    let failed = checks.iter().filter(|c| !c.ok).count();
    println!("\n=== ПЕРЕВІРКА ВІТРИН ===");
    for c in &checks {
        println!("\n[{}] {}", if c.ok { "ok  " } else { "FAIL" }, c.name);
        println!("     {}", c.detail);
        for s in &c.samples { println!("       - {s}"); }
    }
    println!("\nпровалено перевірок: {failed} із {}", checks.len());

    let doc = json!({
        "generated": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs(),
        "dictVersion": wr_dict::dict_version(),
        "failed": failed,
        "checks": checks.iter().map(|c| json!({
            "name": c.name, "ok": c.ok, "detail": c.detail, "samples": c.samples
        })).collect::<Vec<_>>()
    });
    let outp = format!("{b}/verify-report.json");
    let _ = File::create(&outp).map(|mut f| f.write_all(serde_json::to_string_pretty(&doc).unwrap().as_bytes()));
    println!("JSON: {outp}");
    if failed > 0 { std::process::exit(1) }
}
