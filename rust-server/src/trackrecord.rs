//! `/api/track-record` — послужний список сигналів: чи вони працювали.
//!
//! ЧОМУ ЦЕ Є В ПРОДУКТІ. Жоден інструмент цього ринку не показує, чи справдились його
//! власні сигнали. Nansen позначає гаманець «розумним» за МИНУЛУ дохідність; Bubblemaps
//! не публікує ваг свого скора; провідний форензик-вендор під присягою не зміг назвати
//! рівень помилок свого продукту. Ми можемо — бо кожне наше твердження з моменту
//! народження лежить у журналі разом із ціною, і дозріває за розкладом.
//!
//! ГОЛОВНЕ, ЩО ТУТ ПОКАЗУЄТЬСЯ, — НЕ ЧАСТКА ВЛУЧАНЬ. Сама по собі вона обманює:
//! «вгадано падіння 72%» звучить переконливо, поки не побачиш, що випадковий вхід у ті
//! самі активи дає 74%. Тому поруч із кожним числом стоїть нульова база, і присуд
//! виноситься лише за різницею між ними.
//!
//! Джерело правди — `wr_store::trackrecord`: той самий код рахує щодобовий звіт у
//! Telegram. Тут лише читання файлів, кеш і подача.

use crate::state::Ctx;
use axum::{extract::State, response::IntoResponse, Json};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use wr_store::journal::{Journal, Outcome, Signal};
use wr_store::trackrecord::{summarize, Opts};

/// Журнал живий (`dataset/`) і ретро (`dataset/retro/`). Ретро — це реплей детектора по
/// історії біржі: він дає присуд одразу, але має перекоси виживання й зазирання вперед,
/// тож змішувати його з живим в одну купку не можна. Вони й не змішуються: різні
/// джерела сигналів дають різні рядки підсумку.
struct Loaded {
    signals: Vec<Signal>,
    outcomes: Vec<Outcome>,
    null_base: Value,
    bad_lines: usize,
}

/// Ключ свіжості: розмір і час зміни кожного файла. Журнали важать десятки мегабайт,
/// читати їх на кожен запит не можна; але й кеш із таймером збрехав би одразу після
/// нового прогону дозрівання. Тому перечитуємо саме тоді, коли файли змінились.
fn stamp(paths: &[PathBuf]) -> Vec<(u64, i64)> {
    paths.iter().map(|p| {
        std::fs::metadata(p).ok().map(|m| {
            let mt = m.modified().ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64).unwrap_or(0);
            (m.len(), mt)
        }).unwrap_or((0, 0))
    }).collect()
}

fn load(dir: &Path) -> Loaded {
    let retro = dir.join("retro");
    let mut signals = Vec::new();
    let mut outcomes = Vec::new();
    let mut bad = 0usize;

    for d in [dir.to_path_buf(), retro.clone()] {
        match Journal::open_at(&d) {
            Ok(mut j) => {
                match j.signals() { Ok(v) => signals.extend(v), Err(e) => tracing::warn!("[track-record] {}: {e}", d.display()) }
                match j.outcomes() { Ok(v) => outcomes.extend(v), Err(e) => tracing::warn!("[track-record] {}: {e}", d.display()) }
                bad += j.bad_lines();
            }
            // Відсутність ретро-журналу — нормальний стан (реплей ще не ганяли),
            // а не помилка. Відсутність живого — теж: він наповнюється сам.
            Err(e) => tracing::debug!("[track-record] {} недоступний: {e}", d.display()),
        }
    }

    let null_base = std::fs::read_to_string(retro.join("retro-null.json")).ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Value::Null);

    Loaded { signals, outcomes, null_base, bad_lines: bad }
}

fn now_s() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64).unwrap_or(0)
}

type Cache = Mutex<Option<(Vec<(u64, i64)>, Value)>>;

fn cache() -> &'static Cache {
    static C: OnceLock<Cache> = OnceLock::new();
    C.get_or_init(|| Mutex::new(None))
}

/// Зібрати підсумок (з кешем за свіжістю файлів).
pub fn build(dir: &Path) -> Value {
    let files = vec![
        dir.join("signals.jsonl"),
        dir.join("outcomes.jsonl"),
        dir.join("retro").join("signals.jsonl"),
        dir.join("retro").join("outcomes.jsonl"),
        dir.join("retro").join("retro-null.json"),
    ];
    let key = stamp(&files);

    if let Ok(g) = cache().lock() {
        if let Some((k, v)) = g.as_ref() {
            if *k == key { return v.clone() }
        }
    }

    let l = load(dir);
    let opts = Opts { now: now_s(), ..Default::default() };
    let s = summarize(&l.signals, &l.outcomes, &l.null_base, &opts);

    // Вік даних на екрані — обов'язковий: найчастіша претензія до всіх інструментів
    // ринку саме в тому, що цифра свіжа на вигляд і тримісячна насправді.
    let ages: Vec<Value> = files.iter().zip(key.iter()).map(|(p, (len, mt))| json!({
        "file": p.file_name().and_then(|x| x.to_str()).unwrap_or(""),
        "path": p.parent().and_then(|x| x.file_name()).and_then(|x| x.to_str()).unwrap_or(""),
        "bytes": len,
        "mtime": mt,
        "ageSec": if *mt > 0 { now_s() - mt } else { -1 },
    })).collect();

    let v = json!({
        "asOf": now_s(),
        "minN": opts.min_n,
        "benchSymbol": opts.bench_sym,
        "summary": s,
        "badLines": l.bad_lines,
        "files": ages,
        // Рамка, без якої числа читаються неправильно. Її треба показувати поруч із
        // таблицею, а не ховати у виносці.
        "caveats": [
            "Присуд виноситься проти ВИПАДКОВОГО входу в той самий актив на тому самому горизонті, а не проти нуля.",
            "Похибка рахується на кількості активів, а не сигналів: вікна сусідніх сигналів одного токена перекриваються.",
            "Рядки retro-* — реплей детектора по історії: вибірка складена з активів, які групи тримають СЬОГОДНІ (виживання), а частка емісії рахована з сьогоднішнього випуску (зазирання вперед).",
            "Базовий актив виключено з вибірки: надлишок активу проти самого себе тотожно нульовий."
        ]
    });

    if let Ok(mut g) = cache().lock() { *g = Some((key, v.clone())) }
    v
}

pub async fn route(State(s): State<Ctx>) -> impl IntoResponse {
    Json(build(s.dataset.root()))
}
