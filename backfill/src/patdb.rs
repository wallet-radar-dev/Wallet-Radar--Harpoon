//! patdb — зібрати ВСЕ вже знайдене в накопичувальну базу патернів і показати зведення.
//!
//! Досі результати кожного аналізу лежали окремим JSON: rolemap.json знав ролі, bullrun.json
//! знав активність, contract-flags.json знав контракти — і ніде не було адреси, про яку видно
//! все відразу. Цей бін імпортує наявні файли у `wallet-patterns.jsonl` (див. `patterns`),
//! після чого будь-який бін питає базу замість того, щоб рахувати заново.
//!
//! Імпорт ідемпотентний: повторний запуск не дублює записи, лише оновлює докази. Тому
//! ганяти його можна після кожного нового аналізу.
//!
//! Args: ROOT [addr]  — з адресою показує картку цієї адреси замість імпорту.
//! Env: SKIP_IMPORT=1 — тільки зведення, без читання джерел.
use backfill::known::{self, pat};
use backfill::patterns::{Finding, PatternDb};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Write};

fn read_json(p: &str) -> Option<Value> {
    serde_json::from_reader(BufReader::with_capacity(1 << 20, File::open(p).ok()?)).ok()
}

fn read_list(p: &str) -> Vec<String> {
    File::open(p)
        .map(|f| {
            BufReader::new(f)
                .lines()
                .map_while(Result::ok)
                .map(|l| l.trim().to_lowercase())
                .filter(|l| l.starts_with("0x") && l.len() == 42)
                .collect()
        })
        .unwrap_or_default()
}

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let bf = format!("{root}/dataset/backfill");
    let mut db = PatternDb::load(&bf);
    eprintln!("[patdb] у базі вже {} адрес", db.map.len());

    if let Some(q) = std::env::args().nth(2) {
        card(&db, &q.to_lowercase());
        return;
    }

    // ── ПРИЗНАЧЕННЯ: одне слово на гаманець ───────────────────────────────────
    //
    // Патернів на адресу назбиралось багато, і вони не суперечать один одному: адреса
    // може бути MAKER за формою потоку, BUYER за напрямком і HOLDER за залишком одночасно.
    // Але на питання «що це за гаманець» треба відповідати одним словом, тож правила
    // впорядковані за силою свідчення: технічна природа (контракт, біржа) сильніша за
    // поведінку, а поведінка сильніша за розмір.
    //
    // Ключове розрізнення — ТРАНЗИТ проти МАРКЕТ-МЕЙКЕРА. Обидва мають великий обіг і
    // майже нульовий залишок, тож за розміром вони нерозрізненні. Розрізняє СИМЕТРІЯ:
    // маркет-мейкер ганяє туди-назад з тим самим контрагентом (симетрія висока), транзит
    // приймає з одного боку й віддає в інший (симетрія низька).
    if std::env::var("PURPOSE").is_ok() {
        let flow: Value = std::fs::read_to_string(format!("{bf}/flowdir.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or(Value::Null);
        let mut fmap: HashMap<String, (f64, f64, f64)> = HashMap::new(); // addr -> (to, from, sym)
        for r in flow["rows"].as_array().map(|x| x.as_slice()).unwrap_or(&[]) {
            if let Some(a) = r["addr"].as_str() {
                fmap.insert(
                    a.to_lowercase(),
                    (
                        r["toCexUsd"].as_f64().unwrap_or(0.0),
                        r["fromCexUsd"].as_f64().unwrap_or(0.0),
                        r["symmetry"].as_f64().unwrap_or(0.0),
                    ),
                );
            }
        }
        eprintln!("[patdb] потоків із flowdir: {}", fmap.len());

        let mut tally: std::collections::BTreeMap<&str, usize> = Default::default();
        let mut usd_by: std::collections::BTreeMap<&str, f64> = Default::default();
        let mut out: Vec<Value> = Vec::new();

        for (a, recs) in &db.map {
            let has = |p: &str| recs.iter().any(|r| r.pat == p);
            // сумарний підтверджений залишок: докази лежать по-токенно (usd:LINK, usd:PEPE)
            let held: f64 = recs
                .iter()
                .filter(|r| r.pat == pat::HOLDER)
                .flat_map(|r| r.ev.iter())
                .filter(|(k, _)| k.starts_with("usd:"))
                .filter_map(|(_, v)| v.as_f64())
                .sum();
            let (to_cex, from_cex, sym) = fmap.get(a).copied().unwrap_or((0.0, 0.0, 0.0));
            let turnover = to_cex + from_cex;
            let net = to_cex - from_cex;
            // скільки разів обіг перевищує залишок: у тримача це одиниці, у транзиту сотні
            let churn = if held > 1000.0 { turnover / held } else if turnover > 0.0 { 999.0 } else { 0.0 };

            let purpose = if has(pat::CEX_LABELED) || has("CEX-DEPOSIT-ADDR") {
                "БІРЖА"
            } else if has("SAFE") {
                "КАЗНА-МУЛЬТИПІДПИС"
            } else if has(pat::CONTRACT) {
                "КОНТРАКТ"
            } else if has("SAFE-OWNER") && held < 1_000_000.0 && turnover < 1_000_000.0 {
                // Підписант — це ключ, а не гаманець: тримає газ і нічого більше. Саме тому
                // його не бачив жоден відбір за розміром. Але право підпису й власний
                // капітал не виключають одне одного: `0x176f3dab…` підписує казну І тримає
                // $1.12 млрд. Без цієї межі правило перебивало його справжню роль.
                "ПІДПИСАНТ"
            } else if turnover > 100_000.0 && churn > 10.0 && sym >= 0.5 {
                "МАРКЕТ-МЕЙКЕР"
            } else if turnover > 100_000.0 && churn > 10.0 && sym < 0.1 {
                "ТРАНЗИТ"
            } else if has(pat::TREASURY) || has("SHARED-FUNDER") {
                "КАЗНА"
            } else if has(pat::DISTRIBUTOR) || has(pat::FUNDER) && turnover < 10_000.0 {
                "РОЗДАВАЧ"
            } else if net > 1_000_000.0 {
                "РОЗВАНТАЖУЄ"
            } else if -net > 1_000_000.0 {
                "НАБИРАЄ"
            } else if held > 1_000_000.0 {
                "ТРИМАЧ"
            } else if has(pat::DORMANT) {
                "СПЛЯЧИЙ"
            } else if has(pat::FUNNEL) && held < 100_000.0 {
                // «воронка» без капіталу — це жертва спам-роздач, а не збирач
                "ЖЕРТВА-СПАМУ"
            } else if held > 10_000.0 || turnover > 10_000.0 {
                "ДРІБНИЙ-АКТИВНИЙ"
            } else if held == 0.0 && turnover == 0.0 && has("ACTIVITY") {
                // Адреса рухається, але ані цін для її токенів, ані біржових потоків
                // для неї немає. Це НЕ «незначний»: «незначний» означає «виміряли й
                // воно мале», а тут ми не виміряли. Різниця принципова — саме на ній
                // трималася половина зауважень аудиту даних.
                "АКТИВНИЙ-БЕЗ-ОЦІНКИ"
            } else {
                "НЕЗНАЧНИЙ"
            };

            *tally.entry(purpose).or_insert(0) += 1;
            *usd_by.entry(purpose).or_insert(0.0) += held;
            let mut groups: Vec<&str> = Vec::new();
            if has("GRP-6864") {
                groups.push("6864")
            }
            if has("GRP-NEW") {
                groups.push("NEW")
            }
            out.push(json!({"addr": a, "purpose": purpose, "heldUsd": held,
                "toCexUsd": to_cex, "fromCexUsd": from_cex, "symmetry": sym,
                "churn": churn, "groups": groups}));
        }

        println!("\n=== ПРИЗНАЧЕННЯ ГАМАНЦІВ ({} адрес) ===", out.len());
        println!("{:<22} {:>8} {:>18}", "ПРИЗНАЧЕННЯ", "АДРЕС", "ТРИМАЮТЬ $");
        let mut tv: Vec<(&&str, &usize)> = tally.iter().collect();
        tv.sort_by(|a, b| b.1.cmp(a.1));
        for (p, n) in tv {
            println!("{:<22} {:>8} {:>18.0}", p, n, usd_by.get(*p).copied().unwrap_or(0.0));
        }

        // ── розріз по групах капіталу ─────────────────────────────────────────
        // Одна таблиця на всіх ховає головне: чи дві мережі влаштовані однаково.
        // 234 адреси належать обом — їх рахуємо окремо, бо саме вони і є стик.
        println!("\n=== ПРИЗНАЧЕННЯ В РОЗРІЗІ ГРУП ===");
        println!("{:<22} {:>9} {:>9} {:>7} {:>9}", "ПРИЗНАЧЕННЯ", "6864", "НОВА", "ОБИДВІ", "ПОЗА");
        let mut per: std::collections::BTreeMap<&str, [usize; 4]> = Default::default();
        for r in &out {
            let p = r["purpose"].as_str().unwrap_or("");
            let g = r["groups"].as_array().map(|x| x.len()).unwrap_or(0);
            let old = r["groups"].as_array().map(|a| a.iter().any(|v| v == "6864")).unwrap_or(false);
            let new = r["groups"].as_array().map(|a| a.iter().any(|v| v == "NEW")).unwrap_or(false);
            let e = per.entry(match p {
                "БІРЖА" => "БІРЖА",
                x => x,
            })
            .or_insert([0; 4]);
            if g == 2 {
                e[2] += 1;
            } else if old {
                e[0] += 1;
            } else if new {
                e[1] += 1;
            } else {
                e[3] += 1;
            }
        }
        let mut pv: Vec<(&&str, &[usize; 4])> = per.iter().collect();
        pv.sort_by(|a, b| (b.1[0] + b.1[1] + b.1[2]).cmp(&(a.1[0] + a.1[1] + a.1[2])));
        for (p, c) in pv {
            if c[0] + c[1] + c[2] == 0 {
                continue;
            }
            println!("{:<22} {:>9} {:>9} {:>7} {:>9}", p, c[0], c[1], c[2], c[3]);
        }

        let _ = File::create(format!("{bf}/wallet-purpose.json"))
            .unwrap()
            .write_all(serde_json::to_string(&json!({"rows": out})).unwrap().as_bytes());
        eprintln!("[patdb] збережено wallet-purpose.json");
        return;
    }

    if std::env::var("CLEAN_LEGACY").is_ok() {
        let n = db.drop_ev_keys(&["sym", "usd", "emissionPct"]);
        eprintln!("[patdb] прибрано застарілих ключів доказів: {n}");
        db.save(&bf);
        return;
    }

    if std::env::var("SKIP_IMPORT").is_err() {
        // ── контракти (codecheck) ──────────────────────────────────────────────
        if let Some(v) = read_json(&format!("{bf}/contract-flags.json")) {
            let mut n = 0;
            for (a, is_c) in v.as_object().map(|m| m.iter().collect::<Vec<_>>()).unwrap_or_default() {
                if is_c.as_bool() == Some(true) {
                    db.add(a, Finding::new(pat::CONTRACT, "codecheck"));
                    n += 1;
                }
            }
            eprintln!("[patdb] контрактів: {n}");
        }

        // ── мітки бірж ─────────────────────────────────────────────────────────
        if let Some(v) = read_json(&format!("{bf}/labels-cache.json")) {
            let mut n = 0;
            for (a, o) in v.as_object().map(|m| m.iter().collect::<Vec<_>>()).unwrap_or_default() {
                let mut s = o["name"].as_str().unwrap_or("").to_string();
                if let Some(arr) = o["labels"].as_array() {
                    for x in arr {
                        if let Some(t) = x.as_str() {
                            s.push(' ');
                            s.push_str(t)
                        }
                    }
                }
                if known::is_cex_label(&s) {
                    db.add(a, Finding::new(pat::CEX_LABELED, "labels").evs("name", s.trim()));
                    n += 1;
                }
            }
            eprintln!("[patdb] адрес із міткою біржі: {n}");
        }

        // ── ролі (rolemap) ─────────────────────────────────────────────────────
        if let Some(v) = read_json(&format!("{bf}/rolemap.json")) {
            let mut n = 0;
            for r in v["rows"].as_array().map(|x| x.as_slice()).unwrap_or(&[]) {
                let (Some(a), Some(role)) = (r["addr"].as_str(), r["role"].as_str()) else { continue };
                let Some(code) = pat::from_role(role) else { continue };
                db.add(
                    a,
                    Finding::new(code, "rolemap")
                        .ev("usdIn", r["usdIn"].as_f64().unwrap_or(0.0))
                        .ev("usdOut", r["usdOut"].as_f64().unwrap_or(0.0))
                        .ev("senders", r["senders"].as_f64().unwrap_or(0.0))
                        .ev("recips", r["recips"].as_f64().unwrap_or(0.0)),
                );
                // фандер видно з поля funder — це окреме спостереження про ІНШУ адресу
                if let Some(f) = r["funder"].as_str() {
                    if f.starts_with("0x") && f.len() == 42 {
                        db.add(f, Finding::new(pat::FUNDER, "rolemap"));
                    }
                }
                n += 1;
            }
            eprintln!("[patdb] ролей з rolemap: {n}");
        }

        // ── активність у булран ────────────────────────────────────────────────
        if let Some(v) = read_json(&format!("{bf}/bullrun.json")) {
            let mut n = 0;
            for r in v["rows"].as_array().map(|x| x.as_slice()).unwrap_or(&[]) {
                let (Some(a), Some(cat)) = (r["addr"].as_str(), r["cat"].as_str()) else { continue };
                if cat != "АКТИВНИЙ" {
                    continue;
                }
                db.add(
                    a,
                    Finding::new(pat::BULLRUN_ACTIVE, "bullrun")
                        .ev("opsBull", r["opsBull"].as_f64().unwrap_or(0.0))
                        .ev("netUsd", r["netUsd"].as_f64().unwrap_or(0.0)),
                );
                n += 1;
            }
            eprintln!("[patdb] активних у булран: {n}");
        }

        // ── членство в мережах ─────────────────────────────────────────────────
        for (file, code) in [
            (format!("{bf}/net-members.txt"), "GRP-6864"),
            ("/mnt/nvme-disk/wallet-backfill/newgroup-full.txt".to_string(), "GRP-NEW"),
        ] {
            let l = read_list(&file);
            for a in &l {
                db.add(a, Finding::new(code, "membership"));
            }
            if !l.is_empty() {
                eprintln!("[patdb] {code}: {}", l.len());
            }
        }

        // ── диригенти нової групи ──────────────────────────────────────────────
        for a in read_list("/mnt/nvme-disk/wallet-backfill/newgroup-addrs.txt").iter().take(50) {
            db.add(a, Finding::new(pat::CONDUCTOR, "conductor_finder"));
        }

        // ── відомі артефакти ───────────────────────────────────────────────────
        for (a, why) in known::artifact_addrs() {
            db.add(&a, Finding::new(pat::ARTIFACT, "known").evs("why", &why));
        }

        db.save(&bf);
    }

    // ── зведення ───────────────────────────────────────────────────────────────
    println!("\n=== ПАТЕРНИ В БАЗІ ({} адрес) ===", db.map.len());
    for (p, n) in db.tally() {
        println!("  {p:<16} {n:>7}");
    }

    // адреси, про які знаємо найбільше — з них корисно починати ручний розбір
    let mut rich: Vec<(&String, usize)> = db.map.iter().map(|(a, r)| (a, r.len())).collect();
    rich.sort_by(|a, b| b.1.cmp(&a.1));
    println!("\n=== НАЙБІЛЬШЕ СПОСТЕРЕЖЕНЬ НА АДРЕСУ ===");
    for (a, n) in rich.iter().take(15) {
        let ps: Vec<&str> = db.pats(a);
        println!("  {a} · {n} · {}", ps.join(" "));
    }
}

fn card(db: &PatternDb, a: &str) {
    let Some(recs) = db.map.get(a) else {
        println!("про {a} у базі нічого немає");
        return;
    };
    println!("\n=== {a} ===");
    for r in recs {
        let ev: Vec<String> = r.ev.iter().map(|(k, v)| format!("{k}={v}")).collect();
        println!("  {:<16} (від {}, ×{}) {}", r.pat, r.src, r.n, ev.join(" "));
    }
    let mut uniq: Vec<&str> = recs.iter().map(|r| r.pat.as_str()).collect();
    uniq.sort_unstable();
    uniq.dedup();
    let multi: Vec<&&str> = uniq.iter().filter(|p| db.support(a, p) >= 2).collect();
    if !multi.is_empty() {
        println!("  ПІДТВЕРДЖЕНО ≥2 незалежними бінами: {:?}", multi);
    }
}
