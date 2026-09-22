//! emission_dossier (Rust) — порт `python3 -` інлайну з deploy/council-emission{,2}.sh 1:1.
//! Збирає досьє «викуплена емісія + групи + нуль-тест» із emission-buyback.json та emgroups.json,
//! друкує markdown у stdout (скрипт редіректить у deploy/emission-dossier.md для ради uniproxy).
//! Аргумент 1 — корінь (дефолт "." — скрипти роблять cd /opt/wallet-tracker перед викликом).
use serde_json::Value;
use std::fs;

fn read_json(path: &str) -> Value {
    let s = fs::read_to_string(path).unwrap_or_else(|e| panic!("не читається {path}: {e}"));
    serde_json::from_str(&s).unwrap_or_else(|e| panic!("не парситься {path}: {e}"))
}

fn i(v: &Value) -> i64 {
    v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)).unwrap_or(0)
}
fn f(v: &Value) -> f64 {
    v.as_f64().unwrap_or(0.0)
}
/// Python-зріз рядка `[:n]` — по СИМВОЛАХ, не байтах.
fn chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}
fn sector(v: &Value) -> String {
    v["sector"].as_str().unwrap_or("?").to_string()
}

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let e = read_json(&format!("{root}/dataset/backfill/emission-buyback.json"));
    let g = read_json(&format!("{root}/dataset/backfill/emgroups.json"));
    let n = &g["null"];
    let mut p: Vec<String> = Vec::new();

    p.push("ФАКТИ (усе виміряно, не припущення).\n".into());
    p.push(format!(
        "A. ВИКУПЛЕНА ЕМІСІЯ. Всесвіт топ-1000 за капіталізацією; 512 з них ERC-20 на Ethereum; холдери зібралися по {}. Поріг: >40% емісії на ПРИВАТНИХ гаманцях (спалене з обігу і зі знаменника; кастодія бірж і контракти виключені). Пройшли {} активи.\n",
        i(&e["scanned"]),
        i(&e["hits"])
    ));

    let empty: Vec<Value> = Vec::new();
    let assets = e["assets"].as_array().unwrap_or(&empty);
    let top: Vec<&Value> = assets
        .iter()
        .filter(|a| i(&a["holdersOnChain"]) >= 500)
        .take(12)
        .collect();
    p.push("Топ із реальною базою власників (>=500 холдерів):".into());
    for a in &top {
        p.push(format!(
            "  {} ({}) {:.1}% EOA, {} холдерів, до 40% вистачає {} гаманців, сектор {}",
            a["symbol"].as_str().unwrap_or(""),
            chars(a["name"].as_str().unwrap_or(""), 28),
            f(&a["eoaPct"]),
            i(&a["holdersOnChain"]),
            i(&a["holdersTo40"]),
            sector(a)
        ));
    }

    let tiny = assets.iter().filter(|a| i(&a["holdersOnChain"]) < 100).count();
    p.push(format!(
        "\nУВАГА: {} із {} активів мають <100 холдерів узагалі (BSPX=4, SAFO=6, BCAP=10). Їхні 100% - це інституційні токенізовані фонди, які нiколи не розподілялись, а не викуп.\n",
        tiny,
        i(&e["hits"])
    ));

    let groups = g["groups"].as_array().unwrap_or(&empty);
    p.push(format!(
        "B. ГРУПИ. Гаманців із >=3 спільними активами: {}. Знайдено груп: {}.",
        i(&g["walletsMultiAsset"]),
        groups.len()
    ));
    for x in groups {
        p.push(format!(
            "  {}: {} гаманців, реальні гроші ${:.1}М (стейбли ${:.1}М, ETH ${:.1}М, інші токени ${:.1}М), паперова оцінка ${:.1}М, синхронний вхід: {}",
            x["id"].as_str().unwrap_or(""),
            i(&x["size"]),
            f(&x["realUsd"]) / 1e6,
            f(&x["stableUsd"]) / 1e6,
            f(&x["ethUsd"]) / 1e6,
            f(&x["otherTokenUsd"]) / 1e6,
            f(&x["paperUsd"]) / 1e6,
            if x["coordinated"].as_bool().unwrap_or(false) { "ТАК" } else { "НІ" }
        ));
        let sa: Vec<String> = x["sharedAssets"]
            .as_array()
            .unwrap_or(&empty)
            .iter()
            .take(8)
            .map(|s| {
                format!(
                    "{}({},тримають {})",
                    s["symbol"].as_str().unwrap_or(""),
                    sector(s),
                    i(&s["membersHolding"])
                )
            })
            .collect();
        p.push(format!("     кошик: {}", sa.join(", ")));
        let kg: Vec<String> = x["knownGroups"]
            .as_array()
            .unwrap_or(&empty)
            .iter()
            .map(|k| format!("{} x{}", k["group"].as_str().unwrap_or(""), i(&k["n"])))
            .collect();
        p.push(format!("     відоме розбиття: {}", kg.join(", ")));
    }

    p.push(format!(
        "\nC. НУЛЬ-ТЕСТ ({} перестановок зі збереженням степенів гаманець<->актив):",
        i(&n["iters"])
    ));
    p.push(format!(
        "   випадковість дає {:.1} груп у середньому, найбільша {:.1} гаманців (макс {}).",
        f(&n["groupsMean"]),
        f(&n["largestMean"]),
        i(&n["largestMax"])
    ));
    p.push(format!(
        "   спостережено: {} груп, найбільша {} гаманців.",
        i(&n["observedGroups"]),
        i(&n["observedLargest"])
    ));
    p.push("   ВИСНОВОК: випадковий фон дає БІЛЬШИЙ кластер, ніж реальні дані. Поріг ребра (перетин >=3 активів, сумарна вага >=6) НЕ селективний.\n".into());
    p.push("D. ПЕРЕВІРКА РУКАМИ: EG3 нібито мала $1.84 млрд, але $1.82 млрд - це ОДИН гаманець 0x5f7a4c11bd (TBLLX $500М, CRWDX $58М, CRCLX $35М) - підпис емітента серії xStock. Інші двоє тримають $43k і $6.6k. Група розвалилась.".into());
    p.push("E. Входи в спільні активи розкидані на 274-628 днів. Кошик EG1 (SAND, KNC, ZRX, CELR, RE, BLUR, YGG, COTI) - типові мід-кеп альти сезону 2021.".into());

    println!("{}", p.join("\n"));
}
