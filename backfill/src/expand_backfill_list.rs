//! Rust-порт `features/expand_backfill_list.py`. Зливає family-гаманці (families-map.csv col1,
//! skip header) з backfill-wallets.txt — FAMILY FIRST (пріоритет краулу), дедуп зі збереженням
//! порядку. Пише бекап `.bak_prefamily` + оновлений список. Арг: base (дефолт dataset).

use std::collections::HashSet;
use std::fs;

fn main() {
    let base = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/opt/wallet-tracker/dataset".into());
    let fam_path = format!("{base}/families-map.csv");
    let wl_path = format!("{base}/backfill/backfill-wallets.txt");

    // family members (col1, skip header)
    let fam: Vec<String> = fs::read_to_string(&fam_path)
        .expect("families-map.csv")
        .lines()
        .skip(1)
        .filter_map(|l| {
            let w = l.split(',').next()?.trim().to_lowercase();
            if w.starts_with("0x") && w.len() == 42 {
                Some(w)
            } else {
                None
            }
        })
        .collect();

    let existing: Vec<String> = fs::read_to_string(&wl_path)
        .unwrap_or_default()
        .lines()
        .map(|l| l.trim().to_lowercase())
        .filter(|l| !l.is_empty())
        .collect();

    // family FIRST, then existing; dedup preserving order
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<String> = Vec::new();
    for w in fam.iter().chain(existing.iter()) {
        if !w.is_empty() && seen.insert(w.clone()) {
            out.push(w.clone());
        }
    }

    fs::write(format!("{wl_path}.bak_prefamily"), existing.join("\n") + "\n").expect("bak");
    fs::write(&wl_path, out.join("\n") + "\n").expect("write");

    let fam_uniq: HashSet<&String> = fam.iter().collect();
    let ex_set: HashSet<&String> = existing.iter().collect();
    let new_fam = fam_uniq.iter().filter(|w| !ex_set.contains(**w)).count();
    println!(
        "family wallets: {} ({} new) | existing: {} | union: {}",
        fam_uniq.len(),
        new_fam,
        existing.len(),
        out.len()
    );
}
