//! kbprune — КАРАНТИН застарілих записів бази знань застосунку.
//!
//! Причина: `/api/ask` відповідає п'ятьма мовами з `kb-qa.jsonl`, зібраної 2026-07-23. Після
//! перевірок реальними балансами й supply виявилось, що твердження про контроль емісії (31%, 46%,
//! 49%…) хибні — насправді 0–1.2%. Плюс 125 записів говорять про групи A і B, яких більше нема:
//! вони злилися в NET. Тобто застосунок стверджує спростоване — це гірше, ніж мовчати.
//!
//! Правило: у карантин іде запис, якщо (а) його вимір спирається на емісію/капітал (усі числові
//! виміри), або (б) meta.group вказує на групу, якої нема в поточному capital-groups.json.
//! Лишаються виміри, не привʼязані до цих чисел (звʼязки токенів, архетипи мережі).
//! Карантин НЕ видаляється: пишеться поруч, щоб можна було звірити після перегенерації.
//!
//! Вихід: kb-qa.jsonl (очищена, атомарний своп) + kb-qa.stale-<день>.jsonl
//! Args: ROOT. Env: KEEP_DIMS(connections,network) DRY(0)
use serde_json::Value;
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let keep_dims: HashSet<String> = std::env::var("KEEP_DIMS").unwrap_or_else(|_| "connections,network".into())
        .split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    let dry = std::env::var("DRY").map(|v| v == "1").unwrap_or(false);
    let na = format!("{root}/dataset/nextaction");

    // які групи існують ЗАРАЗ
    let cg: Value = serde_json::from_reader(BufReader::new(File::open(format!("{root}/dataset/capital-groups.json")).expect("capital-groups"))).unwrap();
    let live: HashSet<String> = cg["groups"].as_array().cloned().unwrap_or_default().iter()
        .filter_map(|g| g["id"].as_str().map(|s| s.to_string())).collect();
    eprintln!("[kbprune] живих груп: {} · виміри, що лишаються: {:?}", live.len(), keep_dims);

    let path = format!("{na}/kb-qa.jsonl");
    let (mut keep, mut stale): (Vec<String>, Vec<String>) = (Vec::new(), Vec::new());
    let mut by_dim: std::collections::HashMap<String, (usize, usize)> = std::collections::HashMap::new();
    for line in BufReader::new(File::open(&path).expect("kb-qa.jsonl")).lines().map_while(Result::ok) {
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        let dim = v["dim"].as_str().unwrap_or("?").to_string();
        let grp = v["meta"]["group"].as_str().map(|s| s.to_string());
        let dim_ok = keep_dims.contains(&dim);
        let grp_ok = grp.as_ref().map(|g| live.contains(g)).unwrap_or(true);
        let e = by_dim.entry(dim.clone()).or_insert((0, 0));
        if dim_ok && grp_ok { keep.push(line); e.0 += 1; } else { stale.push(line); e.1 += 1; }
    }
    let mut dv: Vec<(&String, &(usize, usize))> = by_dim.iter().collect();
    dv.sort_by(|a, b| (b.1 .0 + b.1 .1).cmp(&(a.1 .0 + a.1 .1)));
    eprintln!("[kbprune] {:<14} {:>8} {:>10}", "вимір", "лишаю", "карантин");
    for (d, (k, s)) in dv { eprintln!("[kbprune] {:<14} {:>8} {:>10}", d, k, s); }
    eprintln!("[kbprune] РАЗОМ лишаю {} · у карантин {}", keep.len(), stale.len());

    if dry { eprintln!("[kbprune] DRY — нічого не записано"); return; }
    let day = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() / 86400;
    let sp = format!("{na}/kb-qa.stale-{day}.jsonl");
    let mut sf = File::create(&sp).unwrap();
    for l in &stale { let _ = writeln!(sf, "{l}"); }
    // атомарний своп, щоб прод не побачив напівфайл
    let tmp = format!("{na}/kb-qa.tmp");
    { let mut kf = File::create(&tmp).unwrap(); for l in &keep { let _ = writeln!(kf, "{l}"); } }
    fs::rename(&tmp, &path).ok();
    eprintln!("[kbprune] kb-qa.jsonl оновлено · карантин у {sp}");
}
