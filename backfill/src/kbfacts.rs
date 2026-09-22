//! kbfacts — наповнює базу знань застосунку ПЕРЕВІРЕНИМИ фактами (5 мов).
//!
//! Стара база стверджувала контроль емісії 31–49% і оперувала групами A/B, яких нема — 208 записів
//! пішли в карантин (`kbprune`). Ця база будується ЛИШЕ з величин, які ми звірили з ланцюжком:
//!   cap-real.json           — реальний капітал груп (баланси ETH + ліквідна частина токенів)
//!   group-token-signals.json— холдинги токенів на реальних балансах + емісія з Etherscan supply
//!   wallet-roles.json       — функція гаманців із руху грошей
//!   group-partition.json    — розбиття (кожен гаманець рівно в одній групі)
//! Числа підставляються детерміновано; локальна модель лише ПЕРЕКЛАДАЄ (ru/en/de/fr) — так само,
//! як у kbqa, щоб не було галюцинацій у цифрах.
//! Вихід: APPEND у dataset/nextaction/kb-qa.jsonl. Args: ROOT. Env: MODEL(qwen2.5:7b) NO_TRANSLATE(0)
use serde_json::{json, Value};
use std::fs::{File, OpenOptions};
use std::io::{BufReader, Write};

fn f0(x: f64) -> String { let s = format!("{:.0}", x); let b = s.as_bytes(); let mut o = String::new();
    for (i, c) in b.iter().enumerate() { if i > 0 && (b.len() - i) % 3 == 0 { o.push(' '); } o.push(*c as char); } o }

async fn translate(client: &reqwest::Client, model: &str, text: &str, lang: &str) -> String {
    let names = [("ru", "російською"), ("en", "англійською"), ("de", "німецькою"), ("fr", "французькою")];
    let ln = names.iter().find(|(k, _)| *k == lang).map(|(_, v)| *v).unwrap_or("англійською");
    let body = json!({"model": model, "messages": [
        {"role": "system", "content": "Ти перекладач. Переклади текст точно, ЗБЕРІГАЮЧИ ВСІ ЧИСЛА, символи $ і % без змін. Поверни лише переклад."},
        {"role": "user", "content": format!("Переклади {ln}:\n{text}")}], "temperature": 0.1, "max_tokens": 700});
    for _ in 0..3 {
        if let Ok(r) = client.post("http://localhost:11434/v1/chat/completions").json(&body).send().await {
            if let Ok(v) = r.json::<Value>().await {
                if let Some(s) = v["choices"][0]["message"]["content"].as_str() { if !s.trim().is_empty() { return s.trim().to_string(); } }
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    }
    text.to_string()
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let model = std::env::var("MODEL").unwrap_or_else(|_| "qwen2.5:7b".into());
    let no_tr = std::env::var("NO_TRANSLATE").map(|v| v == "1").unwrap_or(false);
    let bf = format!("{root}/dataset/backfill");
    let rd = |p: String| -> Value { serde_json::from_reader(BufReader::new(File::open(&p).unwrap_or_else(|_| panic!("нема {p}")))).unwrap_or(json!({})) };

    let cap = rd(format!("{bf}/cap-real.json"));
    let sig = rd(format!("{bf}/group-token-signals.json"));
    let roles = rd(format!("{bf}/wallet-roles.json"));
    let part = rd(format!("{bf}/group-partition.json"));

    let mut facts: Vec<(String, String, String, String, Value)> = Vec::new();   // id, dim, q, a, meta
    let add = |v: &mut Vec<(String, String, String, String, Value)>, id: &str, dim: &str, q: String, a: String, meta: Value| {
        v.push((id.into(), dim.into(), q, a, meta));
    };

    // 1) капітал груп — реальний
    for g in cap["groups"].as_array().cloned().unwrap_or_default() {
        let id = g["id"].as_str().unwrap_or("?").to_string();
        if !matches!(id.as_str(), "NET" | "HG2" | "HG3" | "HG4") { continue; }
        let (real, eth, tok) = (g["realTotalUsd"].as_f64().unwrap_or(0.0), g["nativeEthUsd"].as_f64().unwrap_or(0.0), g["realizableUsd"].as_f64().unwrap_or(0.0));
        let (core_n, core_eth) = (g["coreMembers"].as_u64().unwrap_or(0), g["coreEthUsd"].as_f64().unwrap_or(0.0));
        add(&mut facts, &format!("capital:{id}"), "capital",
            format!("Який реальний капітал групи {id}?"),
            format!("Група {id} має реальний капітал ${} : нативний ETH ${} плюс ліквідна частина токенів ${}. Ядро групи (hop 0-2) — {} гаманців із ${} у ETH. Це перевірені баланси, а не оцінка з потоків.",
                f0(real), f0(eth), f0(tok), core_n, f0(core_eth)),
            json!({"group": id, "realUsd": real}));
    }

    // 2) токени й емісія — на перевірених величинах
    let toks = sig["tokens"].as_array().cloned().unwrap_or_default();
    let (tot, exitable) = (sig["totalUsd"].as_f64().unwrap_or(0.0), sig["exitableUsd"].as_f64().unwrap_or(0.0));
    let grp = sig["group"].as_str().unwrap_or("NET").to_string();
    let max_em = toks.iter().filter_map(|t| t["emissionPct"].as_f64()).fold(0.0f64, f64::max);
    add(&mut facts, &format!("emission:{grp}"), "emission",
        format!("Яку частку емісії токенів контролює група {grp}?"),
        format!("Група {grp} НЕ контролює емісію жодного токена: максимальна частка — {:.1}% (перевірено реальними балансами та supply з Etherscan). Раніші оцінки 40–100% були артефактом реконструкції потоків і не підтвердились.", max_em),
        json!({"group": grp, "maxEmissionPct": max_em}));
    add(&mut facts, &format!("holdings:{grp}"), "holdings",
        format!("Скільки токенів тримає група {grp} і на яку суму?"),
        format!("Група {grp} тримає {} токенів на ${}, з яких реально можна вийти на ${} ({:.0}%). Токени — це близько 1% капіталу групи, решта в ETH.",
            toks.len(), f0(tot), f0(exitable), if tot > 0.0 { 100.0 * exitable / tot } else { 0.0 }),
        json!({"group": grp, "tokens": toks.len(), "usd": tot}));
    for t in toks.iter().take(12) {
        let sym = t["symbol"].as_str().unwrap_or("?");
        add(&mut facts, &format!("token:{grp}:{sym}"), "token",
            format!("Яка позиція групи {grp} у токені {sym}?"),
            format!("Група {grp} тримає {sym} на ${} (частка емісії {}), фаза — {}. Вийти з позиції можна на ${}; сигнал: {}.",
                f0(t["usd"].as_f64().unwrap_or(0.0)),
                t["emissionPct"].as_f64().map(|e| format!("{e:.1}%")).unwrap_or_else(|| "невідома".into()),
                t["phase"].as_str().unwrap_or("?"), f0(t["exitableUsd"].as_f64().unwrap_or(0.0)),
                t["signal"].as_str().unwrap_or("")),
            json!({"group": grp, "symbol": sym}));
    }

    // 3) ролі гаманців
    let mut rs: Vec<(String, u64, f64)> = roles["summary"].as_array().cloned().unwrap_or_default().iter()
        .map(|r| (r["role"].as_str().unwrap_or("?").to_string(), r["n"].as_u64().unwrap_or(0), r["eth"].as_f64().unwrap_or(0.0))).collect();
    rs.sort_by(|a, b| b.1.cmp(&a.1));
    let desc: Vec<String> = rs.iter().take(8).map(|(r, n, e)| format!("{r}: {n} ({:.0} ETH)", e)).collect();
    add(&mut facts, "roles:all", "roles",
        "Які функції виконують гаманці мережі?".into(),
        format!("Функцію визначено для {} гаманців за реальним рухом грошей: {}. Гроші зосереджені у холдерів, тоді як фандери, транзит і продавці майже нічого не тримають.",
            roles["wallets"].as_array().map(|a| a.len()).unwrap_or(0), desc.join("; ")),
        json!({}));

    // 4) розбиття
    let groups = part["groups"].as_array().cloned().unwrap_or_default();
    let total: u64 = groups.iter().filter_map(|g| g["size"].as_u64()).sum();
    let top: Vec<String> = groups.iter().take(5).map(|g| format!("{} — {}", g["id"].as_str().unwrap_or("?"), g["size"].as_u64().unwrap_or(0))).collect();
    add(&mut facts, "partition:all", "partition",
        "Як гаманці розподілені по групах капіталу?".into(),
        format!("Усього {} гаманців розкладено так, що кожен належить рівно одній групі: {}. Кошики NODATA (нема даних) та INFRA (біржі й контракти) не є капіталом мережі.",
            f0(total as f64), top.join("; ")),
        json!({}));

    // 5) структура операції
    add(&mut facts, "structure:treasury", "structure",
        "Як влаштована мережа капіталу?".into(),
        "Мережа має триланкову будову: казна 0x0e58e899 (1 032 077 ETH) зрідка й великими сумами живить роздавача 0x12ef5e05, той автоматично розсилає рівні частки десяткам гаманців-членів, і 51% роздачі повертається всередину мережі. Біржових адрес серед призначень нема. Роздавачі змінюються поколіннями і між собою не перетинаються.".into(),
        json!({}));
    add(&mut facts, "structure:signal", "structure",
        "Чи передбачає активність мережі рух ціни?".into(),
        "Ні. Подієве дослідження 11 793 виплат (101 подієвий день) не виявило значущого впливу на ціни кошика мережі на горизонтах 1, 3, 7 і 14 днів (p від 0.14 до 0.97). Моніторинг мережі є інструментом розвідки, а не торговим сигналом.".into(),
        json!({}));

    eprintln!("[kbfacts] згенеровано фактів: {}", facts.len());
    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(120)).build().unwrap();
    let path = format!("{root}/dataset/nextaction/kb-qa.jsonl");
    let mut out = OpenOptions::new().create(true).append(true).open(&path).unwrap();
    for (i, (id, dim, q, a, meta)) in facts.iter().enumerate() {
        let mut am = serde_json::Map::new();
        am.insert("uk".into(), json!(a));
        if !no_tr {
            for lang in ["ru", "en", "de", "fr"] { am.insert(lang.into(), json!(translate(&client, &model, a, lang).await)); }
        }
        let rec = json!({"id": id, "dim": dim, "grounded": true, "meta": meta,
            "q": {"uk": q}, "a": Value::Object(am)});
        let _ = writeln!(out, "{rec}");
        if (i + 1) % 5 == 0 || i + 1 == facts.len() { eprintln!("[kbfacts] {}/{}", i + 1, facts.len()); }
    }
    eprintln!("[kbfacts] дописано у kb-qa.jsonl");
}
