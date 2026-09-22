//! ПАТЕРН parity (Фаза 0.2 плану — Aladdin: «усі модулі показують ту саму цифру»).
//! Бере МЕТРИКУ, яку рахують ДВА різні старі біни по тих самих токенах, і звіряє їх.
//! Розбіжність = той самий баг, що словник має закрити (визначення розійшлись між інструментами).
//! Джерело правди визначення — wr-dict; тут показуємо, ДЕ старі вітрини від нього відходять.
//!   Вхід: dataset/capital-group-assets.json (top10pct, groupBought)
//!         dataset/backfill/emission-buyback.json (top10Pct — та сама концентрація)
//!         dataset/backfill/token-supply.json (обіг — авторитетний знаменник)
//!   Аргументи: tol=1.0 (допуск, в.п.)  top=20
use std::collections::HashMap;
use serde_json::Value;
use crate::core::ctx::Ctx;
use crate::core::dict;
use crate::core::PResult;
use super::{Pattern, num};

pub struct Parity;

/// шукає файл у теці даних та у стандартних теках вітрин.
/// Абсолютні кандидати обовʼязкові: dataset/backfill — симлінк на інший диск,
/// тож обхід через `..` веде не туди (у /mnt/nvme-disk, а не в dataset/).
fn find_file(dir: &str, name: &str) -> Option<String> {
    for p in [
        format!("{dir}/{name}"),
        format!("{dir}/../{name}"),
        format!("/opt/wallet-tracker/dataset/{name}"),
        format!("/opt/wallet-tracker/dataset/backfill/{name}"),
    ] {
        if std::path::Path::new(&p).exists() { return Some(p) }
    }
    None
}

impl Pattern for Parity {
    fn name(&self) -> &str { "parity" }
    fn about(&self) -> &str { "звірка однієї метрики між двома старими бінами (Aladdin: одна цифра)" }

    fn run(&self, ctx: &Ctx, args: &std::collections::BTreeMap<String, String>) -> PResult {
        let tol = num(args, "tol", 1.0);            // допуск у відсоткових пунктах
        let top = num(args, "top", 20.0) as usize;

        // ── старий бін A: capital-group-assets.json (top10pct, groupBought) ──
        let pa = find_file(&ctx.dir, "capital-group-assets.json")
            .ok_or("нема capital-group-assets.json")?;
        let va: Value = serde_json::from_str(&std::fs::read_to_string(&pa).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        let assets = va.get("assets").and_then(Value::as_array)
            .ok_or("capital-group-assets.json: нема поля assets")?;
        let mut a_top10: HashMap<String, (String, f64, f64)> = HashMap::new(); // contract -> (sym, top10, groupBought)
        let mut artifacts = 0usize;
        for it in assets {
            let c = it.get("contract").and_then(Value::as_str).unwrap_or("").to_lowercase();
            if c.is_empty() { continue }
            let sym = it.get("symbol").and_then(Value::as_str).unwrap_or("").to_string();
            let t10 = it.get("top10pct").and_then(Value::as_f64).unwrap_or(-1.0);
            let gb = it.get("groupBought").and_then(Value::as_f64).unwrap_or(-1.0);
            // словник: частка емісії ∈ [0,1] -> відсоток ∈ [0,100]. Поза межами = артефакт визначення.
            if gb >= 0.0 && dict::emission_share(gb, 100.0) > 1.0 { artifacts += 1; }
            a_top10.insert(c, (sym, t10, gb));
        }

        // ── старий бін B: emission-buyback.json (top10Pct — та сама метрика) ──
        let pb = find_file(&ctx.dir, "emission-buyback.json")
            .ok_or("нема emission-buyback.json")?;
        let vb: Value = serde_json::from_str(&std::fs::read_to_string(&pb).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        // записи лежать під ключем "all" (усі токени) чи "assets"; підстраховка — корінь-масив
        let empty = vec![];
        let b_list = vb.get("all").and_then(Value::as_array)
            .or_else(|| vb.get("assets").and_then(Value::as_array))
            .or_else(|| vb.as_array())
            .unwrap_or(&empty);
        let mut b_top10: HashMap<String, f64> = HashMap::new();
        for it in b_list {
            let c = it.get("contract").and_then(Value::as_str).unwrap_or("").to_lowercase();
            let t10 = it.get("top10Pct").and_then(Value::as_f64).unwrap_or(-1.0);
            if !c.is_empty() && t10 >= 0.0 { b_top10.insert(c, t10); }
        }

        // ── обіг як авторитетний знаменник (token-supply.json) ──
        let supply_known = find_file(&ctx.dir, "token-supply.json")
            .and_then(|p| std::fs::read_to_string(&p).ok())
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .and_then(|v| v.as_object().map(|o| o.keys().map(|k| k.to_lowercase()).collect::<std::collections::HashSet<_>>()))
            .unwrap_or_default();

        // ── ЗВІРКА: контракти в обох бінах ──
        let mut diverge: Vec<(String, String, f64, f64, f64, bool)> = vec![]; // sym, contract, A, B, |Δ|, supply?
        let mut agree = 0usize;
        for (c, (sym, ta, _gb)) in &a_top10 {
            if *ta < 0.0 { continue }
            if let Some(&tb) = b_top10.get(c) {
                let d = (ta - tb).abs();
                if d > tol {
                    diverge.push((sym.clone(), c.clone(), *ta, tb, d, supply_known.contains(c)));
                } else { agree += 1; }
            }
        }
        diverge.sort_by(|x, y| y.4.partial_cmp(&x.4).unwrap());

        // ── ЗВІТ ──
        println!("── ПАРИТЕТ (Aladdin: одна цифра, dict {}) ──", dict::dict_version());
        println!("метрика: top-10 концентрація (частка емісії топ-10 холдерів)");
        println!("бін A: capital-group-assets.json ({} активів)", a_top10.len());
        println!("бін B: emission-buyback.json ({} активів)", b_top10.len());
        let compared = agree + diverge.len();
        println!("спільних контрактів: {compared} · збіг (±{tol}в.п.): {agree} · РОЗБІЖНІСТЬ: {}", diverge.len());
        if artifacts > 0 { println!("⚠ артефактів визначення (groupBought поза [0,100]): {artifacts}"); }
        if diverge.is_empty() {
            println!("✓ де порівнянно — цифри сходяться; словник є єдиним визначенням");
        } else {
            println!("── найбільші розбіжності (worklist на міграцію) ──");
            for (sym, c, ta, tb, d, sup) in diverge.iter().take(top) {
                println!("  {:<8} A={:.1}%  B={:.1}%  Δ={:.1}в.п. {}  {}",
                    sym, ta, tb, d, if *sup {"[supply✓]"} else {"[supply?]"}, &c[..10.min(c.len())]);
            }
            println!("→ ці токени рахуються двома бінами по-різному: перевести обидва на wr_dict-визначення");
        }
        Ok(())
    }
}
