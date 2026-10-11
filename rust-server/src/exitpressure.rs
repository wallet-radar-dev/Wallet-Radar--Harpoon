//! `/api/exit-pressure` — хто намагається вийти з позиції, з якої вийти не може.
//!
//! ПИТАННЯ, НА ЯКЕ ВІДПОВІДАЄ. Велика позиція сама по собі нічого не значить: кит на
//! $10 млн у ліквідному токені виходить за годину й ринок цього не помічає. Значення має
//! відношення позиції до ОБСЯГУ: якщо група тримає стільки, що на вихід їй потрібні
//! тижні ринкового обороту, — вона в пастці. А якщо вона при цьому вже почала роздавати,
//! то кожен наступний продаж б'є по ціні, і це видно заздалегідь.
//!
//! Цього перетину не показує жоден інструмент ринку: усі міряють розмір позиції в
//! доларах, а не в днях обороту, і жоден не зводить його з фазою (набирає/роздає).
//!
//! ДЖЕРЕЛО. `capital-groups.json` → `groups[].tokenSignals[]`, які пише бін `groupsig`
//! (реальні баланси з ланцюга + випуск + добовий обсяг). Нічого не рахується заново:
//! маршрут лише відбирає, зводить і ранжує.
//!
//! ЩО ВІДКИДАЄТЬСЯ І ЧОМУ. У сигналах трапляються позиції з поламаною десятковою
//! точністю: ZENT у поточному наборі показує $90 трильйонів. Публікувати такі числа не
//! можна — але й мовчки викидати теж: скільки саме позицій відсіяно і з якої причини,
//! стоїть у відповіді. Це та сама дисципліна, що в біні `verify`: число, яке не пройшло
//! перевірку, НЕ публікується, але факт відсіву видно.

use crate::state::Ctx;
use axum::{extract::State, response::IntoResponse, Json};
use serde_json::{json, Value};

/// Стеля здорового глузду для вартості однієї позиції. Жодна група в наших даних не
/// тримає позиції дорожчої за десятки мільярдів; усе вище — артефакт decimals.
const SANE_MAX_USD: f64 = 5e10;

/// Нижня межа, щоб не засмічувати список пилом.
const MIN_USD: f64 = 10_000.0;

/// Мінімальний добовий обсяг, за якого «дні обороту» взагалі щось означають.
///
/// ЧОМУ ЦЕ ПОТРІБНО. Перший прогін цього маршруту поставив на перше місце SFUND із
/// позицією на $412 тис. і «281 956 днів обороту» — тобто 772 роки. Це не пастка кита,
/// а токен без ринку: коли обсяг близький до нуля, відношення позиції до обороту злітає
/// в небо й переважує позиції в сотні мільйонів. Ділення на майже-нуль не є вимірюванням.
/// Такі позиції не ранжуються, а виносяться окремо: «ринку немає» — це теж знахідка,
/// але інша, і плутати її з «кит у пастці» не можна.
const MIN_VOL24: f64 = 1_000.0;

#[derive(Default)]
struct Dropped {
    unpriceable: u64,
    insane_usd: u64,
    no_emission: u64,
    no_volume: u64,
    dust: u64,
}

/// Чи це позиція, про яку взагалі можна щось стверджувати.
fn keep(t: &Value, d: &mut Dropped) -> bool {
    if t["unpriceable"].as_bool() == Some(true) { d.unpriceable += 1; return false }
    let usd = t["usd"].as_f64().unwrap_or(0.0);
    if !usd.is_finite() || usd > SANE_MAX_USD { d.insane_usd += 1; return false }
    if usd < MIN_USD { d.dust += 1; return false }
    match t["emissionPct"].as_f64() {
        Some(p) if p.is_finite() && p > 0.0 => {}
        _ => { d.no_emission += 1; return false }
    }
    // daysOfVolume = -1 означає «обсяг невідомий», а не «виходить миттєво».
    match t["daysOfVolume"].as_f64() {
        Some(v) if v.is_finite() && v >= 0.0 => {}
        _ => { d.no_volume += 1; return false }
    }
    true
}

/// Чи фаза означає роздачу. Рядки приходять із `groupsig` українською.
fn is_distributing(phase: &str) -> bool {
    phase.contains("РОЗДА")
}

/// Оцінка тиску 0..100. Детермінована й із трьох видимих складових, щоб число можна
/// було перевірити очима, а не приймати на віру.
///
/// * скільки днів обороту треба на вихід (логарифмічно: різниця між 1 і 10 днями важить
///   більше, ніж між 100 і 110);
/// * РОЗМІР позиції — без нього перше місце дістається дрібним позиціям у неліквідних
///   токенах, що й сталося в першому прогоні: $56 тис. обійшли сотні мільйонів;
/// * чи група вже роздає — саме тоді пастка стає реальною, а не теоретичною.
fn pressure(days_of_volume: f64, usd: f64, distributing: bool) -> f64 {
    let d = days_of_volume.max(0.0);
    // 1 день → 0.1, 10 днів → 0.35, 100 → 0.67, 1000+ → 1.0
    let trapped = ((d + 1.0).log10() / 3.0).clamp(0.0, 1.0);
    // $100 тис. → 0, $1 млн → 0.33, $10 млн → 0.67, $100 млн і вище → 1.0
    let size = ((usd.max(1.0) / 100_000.0).log10() / 3.0).clamp(0.0, 1.0);
    let base = 100.0 * (0.5 * trapped + 0.5 * size);
    if distributing { (base + 20.0).min(100.0) } else { base }
}

pub fn build(dataset: &crate::dataset::Dataset) -> Value {
    let Some(cg) = dataset.read("capital-groups.json") else {
        return json!({"assets": [], "groups": 0, "note": "capital-groups.json недоступний"});
    };
    let empty = vec![];
    let groups = cg["groups"].as_array().unwrap_or(&empty);

    let mut out: Vec<Value> = Vec::new();
    // Позиції в токенах без ринку: їх не можна ранжувати за днями обороту, але й
    // ховати не можна — «вийти нікуди» це окремий, теж важливий стан.
    let mut no_market: Vec<Value> = Vec::new();
    let mut dropped = Dropped::default();
    let mut groups_with_signals = 0u64;

    for g in groups {
        let sigs = match g["tokenSignals"].as_array() { Some(s) if !s.is_empty() => s, _ => continue };
        groups_with_signals += 1;
        let gid = g["id"].as_str().unwrap_or("").to_string();
        let gname = g["name"].as_str().unwrap_or("").to_string();

        for t in sigs {
            if !keep(t, &mut dropped) { continue }
            let phase = t["phase"].as_str().unwrap_or("").to_string();
            let dist = is_distributing(&phase);
            let dov = t["daysOfVolume"].as_f64().unwrap_or(0.0);
            let usd = t["usd"].as_f64().unwrap_or(0.0);
            let vol24 = t["vol24"].as_f64().unwrap_or(0.0);

            // Ринку немає — «дні обороту» не вимірювані. Окремий список, без оцінки.
            if vol24 < MIN_VOL24 {
                no_market.push(json!({
                    "group": gid, "symbol": t["symbol"].as_str().unwrap_or(""),
                    "contract": t["token"].as_str().unwrap_or(""),
                    "usd": usd, "vol24": vol24,
                    "emissionPct": t["emissionPct"].as_f64(),
                    "phase": phase,
                }));
                continue;
            }

            out.push(json!({
                "group": gid,
                "groupName": gname,
                "symbol": t["symbol"].as_str().unwrap_or(""),
                "contract": t["token"].as_str().unwrap_or(""),
                // Частка емісії — наша лінійка, а не долари.
                "emissionPct": t["emissionPct"].as_f64(),
                "usd": t["usd"].as_f64(),
                // Головне число: скільки ДНІВ ринкового обороту важить позиція.
                "daysOfVolume": dov,
                "vol24": t["vol24"].as_f64(),
                "phase": phase,
                "distributing": dist,
                "net90dUsd": t["net90dUsd"].as_f64(),
                "exitableUsd": t["exitableUsd"].as_f64(),
                "signal": t["signal"].as_str().unwrap_or(""),
                "pressure": (pressure(dov, usd, dist) * 10.0).round() / 10.0,
            }));
        }
    }

    out.sort_by(|a, b| b["pressure"].as_f64().unwrap_or(0.0)
        .partial_cmp(&a["pressure"].as_f64().unwrap_or(0.0)).unwrap_or(std::cmp::Ordering::Equal));

    let trapped_and_selling = out.iter()
        .filter(|a| a["distributing"].as_bool() == Some(true) && a["daysOfVolume"].as_f64().unwrap_or(0.0) >= 7.0)
        .count();

    json!({
        "asOf": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs()).unwrap_or(0),
        "groupsWithSignals": groups_with_signals,
        "groupsTotal": groups.len(),
        "assets": out,
        // Позиції в токенах без ринку: вийти нікуди, і «дні обороту» тут не рахуються.
        "noMarket": no_market,
        // Позиції, де група вже роздає І на вихід треба тиждень обороту й більше.
        "trappedAndSelling": trapped_and_selling,
        // Скільки позицій відсіяно і чому — мовчазний відсів сам по собі був би хибою.
        "dropped": {
            "unpriceable": dropped.unpriceable,
            "insaneUsd": dropped.insane_usd,
            "noEmission": dropped.no_emission,
            "noVolume": dropped.no_volume,
            "dust": dropped.dust,
        },
        "method": "Тиск = скільки днів ринкового обороту важить позиція (логарифмічно) + надбавка, якщо група вже роздає. Обсяг невідомий → позиція не публікується взагалі, а не вважається ліквідною.",
    })
}

pub async fn route(State(s): State<Ctx>) -> impl IntoResponse {
    Json(build(&s.dataset))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_volume_is_not_liquid() {
        let mut d = Dropped::default();
        // daysOfVolume = -1 у groupsig означає «обсяг невідомий»
        let t = json!({"unpriceable": false, "usd": 1e6, "emissionPct": 1.0, "daysOfVolume": -1.0});
        assert!(!keep(&t, &mut d), "невідомий обсяг не можна вважати миттєвим виходом");
        assert_eq!(d.no_volume, 1);
    }

    #[test]
    fn broken_decimals_are_dropped_and_counted() {
        let mut d = Dropped::default();
        let t = json!({"unpriceable": false, "usd": 9.0e13, "emissionPct": 1.0, "daysOfVolume": 1.0});
        assert!(!keep(&t, &mut d));
        assert_eq!(d.insane_usd, 1, "відсів має бути видно у відповіді, а не мовчазним");
    }

    #[test]
    fn distributing_a_heavy_position_outranks_holding_one() {
        let heavy_selling = pressure(44.0, 1e7, true);
        let heavy_holding = pressure(44.0, 1e7, false);
        let light_selling = pressure(0.5, 1e7, true);
        assert!(heavy_selling > heavy_holding, "роздача важкої позиції — найгірший випадок");
        assert!(heavy_selling > light_selling);
    }

    /// Регресія на реальну ваду першого прогону: позиція на $56 тис. у токені без
    /// обороту опинилась вище позицій на сотні мільйонів, бо «дні обороту» злітали
    /// в небо від ділення на майже-нуль. Розмір позиції мусить важити.
    #[test]
    fn a_tiny_position_never_outranks_a_huge_one() {
        let tiny_illiquid = pressure(281_956.0, 56_000.0, true);
        let huge_liquid = pressure(3.0, 5e8, true);
        assert!(huge_liquid > tiny_illiquid,
            "позиція на $500 млн важливіша за $56 тис., хоч би скільки «днів обороту» показував неліквід");
    }

    #[test]
    fn pressure_is_bounded() {
        assert!(pressure(0.0, 0.0, false) >= 0.0);
        assert!(pressure(1e9, 1e12, true) <= 100.0);
    }

    #[test]
    fn phase_word_is_recognised() {
        assert!(is_distributing("РОЗДАЄ"));
        assert!(!is_distributing("ТРИМАЄ"));
        assert!(!is_distributing("НАБИРАЄ"));
    }
}
