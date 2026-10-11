//! trackrecord — підбиття підсумку журналу сигналів: чи вони працювали.
//!
//! НАВІЩО ОКРЕМИЙ МОДУЛЬ. Присуд «сигнал працює / не працює» потрібен у двох місцях:
//! у біні `sigcheck` (щодобовий звіт у Telegram) і в самому застосунку (екран послужного
//! списку). Якби кожен рахував по-своєму, з часом вони б розійшлись — і користувач
//! бачив би на екрані одне, а в повідомленні інше. Тому арифметика присуду живе тут в
//! одному екземплярі, а споживачі лише по-різному її показують.
//!
//! ГОЛОВНЕ ПРАВИЛО МОДУЛЯ: порівнюємо не з нулем і не з 50%, а з тим, що дав би
//! ВИПАДКОВИЙ вхід у той самий актив на тому самому горизонті. Без цього сигнал на
//! активі, який весь період обганяв ринок, виглядає геніальним, хоча не містить
//! жодної інформації. Саме на цьому обпікся перший ретро-прогін: «вгадано падіння 72%»
//! виявилось дрейфом альтів проти ETH, бо випадковий вхід давав 74%.
//!
//! ДРУГЕ ПРАВИЛО: похибка рахується на КІЛЬКОСТІ АКТИВІВ, а не сигналів. Вікна сусідніх
//! сигналів одного токена перекриваються майже повністю, тож 400 спостережень по двох
//! активах — це приблизно два незалежні досліди, а не чотириста.

use crate::journal::{Outcome, Signal};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Налаштування підбиття. Значення за замовчуванням збігаються з бойовими.
#[derive(Debug, Clone)]
pub struct Opts {
    /// Нижче цього числа спостережень присуду не буває.
    pub min_n: usize,
    /// Горизонти, за якими рахується «ще чекає».
    pub horizons: Vec<u32>,
    /// Тікер базового активу: його сигнали виключаються з вибірки, бо надлишок
    /// активу проти самого себе тотожно нульовий.
    pub bench_sym: String,
    /// Поточний час (секунди UTC) — потрібен, щоб відрізнити «ще не дозріло» від «немає результату».
    pub now: i64,
}

impl Default for Opts {
    fn default() -> Self {
        Opts { min_n: 30, horizons: vec![1, 7, 30, 90], bench_sym: "ETH".into(), now: 0 }
    }
}

/// Присуд по одній клітинці (джерело × напрям × горизонт).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Verdict {
    /// Спостережень замало — саме це й кажемо, а не «поки що не працює».
    Insufficient { need: usize },
    /// Нульової бази для цих активів немає, порівнювати нема з чим.
    NoNullBase,
    /// Різниця з випадковим входом не перевищує похибку.
    SameAsRandom { lift_pp: f64, band_pp: f64 },
    Better { lift_pp: f64, band_pp: f64 },
    /// Гірше за випадковий вхід — тобто сигнал, можливо, інвертований.
    Worse { lift_pp: f64, band_pp: f64 },
}

impl Verdict {
    /// Короткий машинний ярлик для UI й сортування.
    pub fn label(&self) -> &'static str {
        match self {
            Verdict::Insufficient { .. } => "insufficient",
            Verdict::NoNullBase => "no-null-base",
            Verdict::SameAsRandom { .. } => "same-as-random",
            Verdict::Better { .. } => "better",
            Verdict::Worse { .. } => "worse",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cell {
    pub source: String,
    pub kind: String,
    pub horizon_d: u32,
    pub n: usize,
    /// Частка влучань сигналу, %.
    pub hit_rate: f64,
    /// Медіана надлишку над базовим активом, %.
    pub median_excess: f64,
    /// Те саме для випадкового входу в ті самі активи, зважене за кількістю сигналів.
    pub null_hit_rate: Option<f64>,
    pub null_median_excess: Option<f64>,
    /// Яку частку вибірки покриває нульова база, %. Менше 95 — привід не довіряти різниці.
    pub null_coverage_pct: f64,
    /// Скільки різних активів дали ці сигнали — це і є ефективний обсяг вибірки.
    pub assets: usize,
    /// Найбільший актив і його частка вибірки, %: якщо він дає більшість, висновок про нього, а не про сигнал.
    pub top_asset: Option<String>,
    pub top_asset_share: f64,
    pub verdict: Verdict,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Summary {
    pub signals: usize,
    pub outcomes: usize,
    /// Горизонти, що вже минули, але результату ще не мають.
    pub pending: usize,
    /// Результати зі статусом не «ok» — ціни не знайшлося.
    pub unresolved: usize,
    /// Скільки записів відкинуто як базовий актив.
    pub excluded_bench: usize,
    pub first_ts: i64,
    pub last_ts: i64,
    pub cells: Vec<Cell>,
    /// Джерела сигналів, що зустрічаються в журналі.
    pub sources: Vec<String>,
}

fn median(v: &mut Vec<f64>) -> f64 {
    if v.is_empty() { return 0.0 }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = v.len();
    if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) / 2.0 }
}

/// Нульова база: `by_tf/<таймфрейм>/<тікер>/<горизонт>` → `{share_up, median_excess}`.
///
/// Ключ включає таймфрейм, бо нуль для годинних відер і для п'ятихвилинних — різні
/// величини; спільний ключ означав би, що пізніший прогін тихо підміняє базу ранішого.
fn null_for(base: &serde_json::Value, tf: &str, sym: &str, horizon: u32) -> Option<(f64, f64)> {
    if tf.is_empty() { return None }
    let v = base.pointer(&format!("/by_tf/{tf}/{sym}/{horizon}"))?;
    Some((v["share_up"].as_f64()?, v["median_excess"].as_f64()?))
}

/// Таймфрейм сигналу: у ретро він у назві джерела (`retro-1h`), у живого — у полі
/// сили (`strength.tf`). Живий монітор працює на тих самих п'ятихвилинках, тому має
/// порівнюватись із тією самою базою, а не лишатись без присуду назавжди.
fn tf_of(s: &Signal) -> String {
    if let Some(tf) = s.source.strip_prefix("retro-") { return tf.to_string() }
    s.strength.get("tf").and_then(|v| v.as_str()).unwrap_or("").to_string()
}

/// Підбити підсумок журналу.
pub fn summarize(
    signals: &[Signal], outcomes: &[Outcome], null_base: &serde_json::Value, opts: &Opts,
) -> Summary {
    let bench = opts.bench_sym.to_uppercase();
    let by_id: HashMap<&str, &Signal> = signals.iter().map(|s| (s.id.as_str(), s)).collect();

    // ключ клітинки → (влучання, надлишки, надлишки понад випадковий вхід, к-ть сигналів на актив)
    struct Acc {
        hits: Vec<f64>,
        excess: Vec<f64>,
        syms: HashMap<String, usize>,
        /// Таймфрейм клітинки — за ним шукається нульова база.
        tf: String,
    }
    let mut cells: HashMap<(String, String, u32), Acc> = HashMap::new();
    let mut unresolved = 0usize;
    let mut excluded = 0usize;

    for o in outcomes {
        let Some(s) = by_id.get(o.id.as_str()) else { continue };
        if s.sym.to_uppercase() == bench { excluded += 1; continue }
        if o.status != "ok" { unresolved += 1; continue }
        let Some(hit) = o.hit(&s.kind) else { continue };
        let key = (s.source.clone(), s.kind.clone(), o.horizon_d);
        let a = cells.entry(key).or_insert_with(|| Acc {
            hits: Vec::new(), excess: Vec::new(), syms: HashMap::new(), tf: tf_of(s),
        });
        a.hits.push(if hit { 1.0 } else { 0.0 });
        a.excess.push(o.excess_pct.unwrap_or(0.0));
        *a.syms.entry(s.sym.clone()).or_insert(0) += 1;
    }

    let mut out: Vec<Cell> = Vec::new();
    for ((source, kind, horizon_d), a) in cells {
        let n = a.hits.len();
        if n == 0 { continue }
        let hit_rate = 100.0 * a.hits.iter().sum::<f64>() / n as f64;
        let mut ex = a.excess.clone();
        let median_excess = median(&mut ex);

        // нульова база, зважена за кількістю сигналів на актив
        let (mut w_sum, mut hit_w, mut med_w) = (0.0f64, 0.0f64, 0.0f64);
        for (sym, cnt) in &a.syms {
            let Some((share_up, med)) = null_for(null_base, &a.tf, sym, horizon_d) else { continue };
            let w = *cnt as f64;
            w_sum += w;
            // для «продажу» влучанням є падіння, тож базу перевертаємо тим самим боком
            hit_w += w * if kind == "sell" { 100.0 - share_up } else { share_up };
            med_w += w * if kind == "sell" { -med } else { med };
        }
        let covered = 100.0 * w_sum / n as f64;
        let (null_hit, null_med) = if w_sum > 0.0 {
            (Some(hit_w / w_sum), Some(med_w / w_sum))
        } else { (None, None) };

        // Ефективний обсяг — кількість АКТИВІВ: вікна сусідніх сигналів одного токена
        // перекриваються, тож вони не є незалежними спостереженнями.
        let assets = a.syms.len();
        let band = 2.0 * 50.0 / (assets.max(1) as f64).sqrt();

        let verdict = if n < opts.min_n {
            Verdict::Insufficient { need: opts.min_n }
        } else if let Some(nh) = null_hit {
            let lift = hit_rate - nh;
            if lift.abs() > band {
                if lift > 0.0 { Verdict::Better { lift_pp: lift, band_pp: band } }
                else { Verdict::Worse { lift_pp: lift, band_pp: band } }
            } else {
                Verdict::SameAsRandom { lift_pp: lift, band_pp: band }
            }
        } else {
            Verdict::NoNullBase
        };

        let top = a.syms.iter().max_by_key(|(_, c)| **c).map(|(s, c)| (s.clone(), 100.0 * *c as f64 / n as f64));

        out.push(Cell {
            source, kind, horizon_d, n, hit_rate, median_excess,
            null_hit_rate: null_hit, null_median_excess: null_med,
            null_coverage_pct: covered,
            assets,
            top_asset: top.as_ref().map(|(s, _)| s.clone()),
            top_asset_share: top.map(|(_, sh)| sh).unwrap_or(0.0),
            verdict,
        });
    }
    out.sort_by(|a, b| (&a.source, &a.kind, a.horizon_d).cmp(&(&b.source, &b.kind, b.horizon_d)));

    // Скільки горизонтів уже минуло, але результату ще немає.
    let have: std::collections::HashSet<(String, u32)> =
        outcomes.iter().map(|o| (o.id.clone(), o.horizon_d)).collect();
    let pending = if opts.now > 0 {
        signals.iter().map(|s| opts.horizons.iter()
            .filter(|&&h| s.ts + h as i64 * 86_400 <= opts.now && !have.contains(&(s.id.clone(), h)))
            .count()).sum()
    } else { 0 };

    let mut sources: Vec<String> = signals.iter().map(|s| s.source.clone()).collect();
    sources.sort();
    sources.dedup();

    Summary {
        signals: signals.len(),
        outcomes: outcomes.len(),
        pending,
        unresolved,
        excluded_bench: excluded,
        first_ts: signals.iter().map(|s| s.ts).min().unwrap_or(0),
        last_ts: signals.iter().map(|s| s.ts).max().unwrap_or(0),
        cells: out,
        sources,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::{signal, Outcome, SCHEMA};
    use serde_json::json;

    fn sig(sym: &str, kind: &str, ts: i64) -> Signal {
        signal("retro-1h", kind, sym, ts, ts + 60, "binance", &format!("{sym}USDT"), 1.0, json!({}))
    }

    fn out(id: &str, h: u32, excess: f64) -> Outcome {
        Outcome {
            v: SCHEMA, id: id.to_string(), horizon_d: h, checked_at: 0,
            price0: Some(1.0), price1: Some(1.0), ret_pct: Some(excess), bench_ret_pct: Some(0.0),
            excess_pct: Some(excess), status: "ok".into(), reason: String::new(),
        }
    }

    /// Ядро модуля: сигнал, що збігається з випадковим входом, не має вважатись робочим.
    #[test]
    fn signal_equal_to_random_entry_is_not_an_edge() {
        let mut sigs = Vec::new();
        let mut outs = Vec::new();
        // 40 сигналів по 40 різних активах, усі «влучили» (+5% надлишку)
        for i in 0..40 {
            let sym = format!("T{i}");
            let s = sig(&sym, "buy", 1000 + i as i64);
            outs.push(out(&s.id, 7, 5.0));
            sigs.push(s);
        }
        // але випадковий вхід у ці самі активи дає такі самі 100% і +5%
        let mut by = serde_json::Map::new();
        for i in 0..40 {
            by.insert(format!("T{i}"), json!({"7": {"share_up": 100.0, "median_excess": 5.0, "n": 1000}}));
        }
        let nb = json!({"by_tf": {"1h": by}});

        let s = summarize(&sigs, &outs, &nb, &Opts::default());
        assert_eq!(s.cells.len(), 1);
        let c = &s.cells[0];
        assert_eq!(c.n, 40);
        assert_eq!(c.hit_rate, 100.0);
        assert_eq!(c.verdict.label(), "same-as-random",
            "100% влучань при 100% у випадкового входу — це не перевага");
    }

    /// А справжня перевага має бути видна.
    #[test]
    fn real_edge_is_detected() {
        let mut sigs = Vec::new();
        let mut outs = Vec::new();
        for i in 0..40 {
            let sym = format!("T{i}");
            let s = sig(&sym, "buy", 1000 + i as i64);
            outs.push(out(&s.id, 7, 5.0));
            sigs.push(s);
        }
        let mut by = serde_json::Map::new();
        for i in 0..40 {
            by.insert(format!("T{i}"), json!({"7": {"share_up": 40.0, "median_excess": -1.0, "n": 1000}}));
        }
        let s = summarize(&sigs, &outs, &json!({"by_tf": {"1h": by}}), &Opts::default());
        assert_eq!(s.cells[0].verdict.label(), "better");
    }

    /// Базовий актив міряв би сам себе — його треба викидати, інакше він тягне все до нуля.
    #[test]
    fn benchmark_asset_is_excluded() {
        let s1 = sig("ETH", "buy", 1000);
        let s2 = sig("LINK", "buy", 1000);
        let outs = vec![out(&s1.id, 7, 0.0), out(&s2.id, 7, 3.0)];
        let s = summarize(&[s1, s2], &outs, &json!({}), &Opts::default());
        assert_eq!(s.excluded_bench, 1);
        assert_eq!(s.cells[0].n, 1, "у вибірці має лишитись лише не-базовий актив");
    }

    /// Мало спостережень — так і кажемо, а не «не працює».
    #[test]
    fn small_sample_gets_no_verdict() {
        let s1 = sig("LINK", "buy", 1000);
        let outs = vec![out(&s1.id, 7, 3.0)];
        let s = summarize(&[s1], &outs, &json!({}), &Opts::default());
        assert_eq!(s.cells[0].verdict.label(), "insufficient");
    }

    /// Без нульової бази присуду не буває — навіть коли спостережень вистачає.
    #[test]
    fn without_null_base_there_is_no_verdict() {
        let mut sigs = Vec::new();
        let mut outs = Vec::new();
        for i in 0..40 {
            let s = sig(&format!("T{i}"), "buy", 1000 + i as i64);
            outs.push(out(&s.id, 7, 5.0));
            sigs.push(s);
        }
        let s = summarize(&sigs, &outs, &json!({}), &Opts::default());
        assert_eq!(s.cells[0].verdict.label(), "no-null-base");
    }

    /// Похибка рахується на активах: 400 сигналів по двох активах — не 400 дослідів.
    #[test]
    fn band_is_computed_on_assets_not_signals() {
        let mut sigs = Vec::new();
        let mut outs = Vec::new();
        for i in 0..200 {
            for sym in ["AAA", "BBB"] {
                let s = sig(sym, "buy", 1000 + i as i64);
                outs.push(out(&s.id, 7, 5.0));
                sigs.push(s);
            }
        }
        let by = json!({"AAA": {"7": {"share_up": 50.0, "median_excess": 0.0, "n": 900}},
                        "BBB": {"7": {"share_up": 50.0, "median_excess": 0.0, "n": 900}}});
        let s = summarize(&sigs, &outs, &json!({"by_tf": {"1h": by}}), &Opts::default());
        let c = &s.cells[0];
        assert_eq!(c.n, 400);
        assert_eq!(c.assets, 2);
        match c.verdict {
            // 2σ на двох активах = ±70 пп: навіть 100% проти 50% не перевищує цю межу
            Verdict::SameAsRandom { band_pp, .. } => assert!(band_pp > 60.0, "похибка на 2 активах має бути широкою, а не ±5"),
            ref v => panic!("очікували «те саме, що випадковий вхід», отримали {:?}", v),
        }
    }

    /// «Продаж» влучає, коли ціна падає — база має перевертатись тим самим боком.
    #[test]
    fn sell_direction_flips_the_null_base() {
        let mut sigs = Vec::new();
        let mut outs = Vec::new();
        for i in 0..40 {
            let s = sig(&format!("T{i}"), "sell", 1000 + i as i64);
            outs.push(out(&s.id, 7, -5.0));   // ціна впала → sell влучив
            sigs.push(s);
        }
        let mut by = serde_json::Map::new();
        for i in 0..40 {
            // випадковий вхід теж падає у 100% випадків
            by.insert(format!("T{i}"), json!({"7": {"share_up": 0.0, "median_excess": -5.0, "n": 1000}}));
        }
        let s = summarize(&sigs, &outs, &json!({"by_tf": {"1h": by}}), &Opts::default());
        let c = &s.cells[0];
        assert_eq!(c.hit_rate, 100.0);
        assert_eq!(c.null_hit_rate, Some(100.0), "для продажу база — це частка ПАДІНЬ");
        assert_eq!(c.verdict.label(), "same-as-random");
    }
}
