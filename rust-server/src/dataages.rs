//! `/api/data-ages` — вік усіх наборів даних одним запитом.
//!
//! НАВІЩО ОКРЕМИЙ МАРШРУТ. Кожна панель тепер несе `_freshness` у власній відповіді
//! (див. `routes::passthrough`), але щоб побачити картину цілком, довелося б стягнути
//! всі 25 файлів — разом це десятки мегабайт. Тут читається лише метадані файлів:
//! відповідь миттєва й важить кілограм тексту.
//!
//! ЧОМУ ЦЕ ВАЖЛИВО. Частина воркерів вимкнена, і їхні файли не оновлюються місяцями, а
//! ендпоінти при цьому відповідають кодом 200 із повним тілом. Найчастіша претензія до
//! інструментів цього ринку — саме така: цифра свіжа на вигляд і тримісячна насправді.
//!
//! ПОРІГ «ЗАСТАРІЛО» ЗАДАЄТЬСЯ ТУТ, поряд із файлом, а не в UI: скільки живуть дані,
//! знає той, хто знає їхнє джерело. Ціни старіють за годину, історія транзакцій — за
//! добу, а результат разового дослідження не старіє взагалі, і позначати його
//! «застарілим» було б хибою.

use crate::state::Ctx;
use axum::{extract::State, response::IntoResponse, Json};
use serde_json::{json, Value};

/// (файл, що це, скільки годин живе; 0 = не старіє за призначенням)
const FILES: &[(&str, &str, i64)] = &[
    // Живі: пишуть увімкнені воркери або біни за розкладом.
    ("capital-groups.json", "Групи капіталу (groupsig, 6 год)", 12),
    ("conductors.json", "Диригенти (воркер, 10 хв)", 3),
    ("active-groups.json", "Активні групи (воркер, 90 хв)", 6),
    ("wallet-asset-scan.json", "Скан активів гаманців (воркер, 3 год)", 12),
    ("whale-finder.json", "Мисливець на китів (воркер, 2 год)", 8),
    ("coordinated-moves.json", "Координовані рухи (воркер, 15 хв)", 3),
    ("history.json", "Часові ряди (dominance, концентрація)", 6),
    ("signals.jsonl", "Журнал сигналів (живий)", 24),
    ("backfill/token-prices.json", "Ціни токенів (tokprices)", 12),
    ("backfill/group-token-signals.json", "Сигнали по токенах груп (groupsig)", 12),
    // Увімкнено 25.09.2026, коли з'ясувалось, що причини вимкнення застаріли:
    // Blockscout відповідає, джерело міток віддає 200. Перший прохід форензики довгий
    // (523 активи), тож її файли ще якийсь час лишаються старими — і це видно тут.
    ("asset-forensics.json", "Форензика активів (воркер, 30 хв)", 48),
    ("whale-clusters.json", "Кластери китів (воркер форензики)", 48),
    ("alpha-feed.json", "Стрічка входів китів (воркер форензики)", 48),
    ("wallet-balances.json", "Баланси гаманців (воркер, 2 год)", 48),
    ("backtest.json", "Бектест (воркер, 3 год)", 168),
    ("labels-cache.json", "Мітки сутностей (воркер, 12 год)", 336),
    // Лишаються вимкненими свідомо: краулер їсть квоту, тренування моделі не на часі.
    ("graph-db.json", "Граф краулера (воркер ВИМКНЕНО: їсть квоту)", 168),
    ("sm-model.json", "Модель smart money (воркер ВИМКНЕНО)", 336),
    // Довідники й разові дослідження: не старіють за призначенням.
    ("token-supply.json", "Реєстр обігу токенів", 0),
    ("backfill/token-liquidity.json", "Ліквідність токенів", 0),
    ("group-assets.json", "Активи груп", 0),
    ("backfill/token-prices-hist.json", "Денні цінові ряди", 0),
    ("retro/retro-null.json", "Нульова база (nullbase)", 0),
    ("backfill/groups-universe.json", "Всесвіт груп (разове)", 0),
    ("backfill/cohort-analysis.json", "Аналіз когорт (разове)", 0),
    ("backfill/master-graph.json", "Мастер-граф десків (разове)", 0),
];

pub fn build(dataset: &crate::dataset::Dataset) -> Value {
    let root = dataset.root();
    let mut items: Vec<Value> = Vec::new();
    let (mut stale, mut missing) = (0u64, 0u64);

    for (file, what, ttl_h) in FILES {
        let p = root.join(file);
        let meta = std::fs::metadata(&p).ok();
        let age_sec: i64 = meta.as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.elapsed().ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(-1);
        let bytes = meta.as_ref().map(|m| m.len()).unwrap_or(0);
        let exists = meta.is_some();
        if !exists { missing += 1 }
        // Порожній файл — окремий випадок: він існує, вік має свіжий, а даних у ньому
        // немає. Саме так виглядає wallet-balances.json на 705 байт.
        let empty = exists && bytes < 512;
        let is_stale = exists && *ttl_h > 0 && age_sec > *ttl_h * 3600;
        if is_stale { stale += 1 }

        items.push(json!({
            "file": file,
            "what": what,
            "exists": exists,
            "bytes": bytes,
            "ageSec": age_sec,
            "ttlHours": ttl_h,
            "stale": is_stale,
            "suspiciouslyEmpty": empty,
        }));
    }

    // Найгірше вгорі: спершу застаріле, далі за віком.
    items.sort_by(|a, b| {
        let sa = a["stale"].as_bool().unwrap_or(false);
        let sb = b["stale"].as_bool().unwrap_or(false);
        sb.cmp(&sa).then(b["ageSec"].as_i64().unwrap_or(0).cmp(&a["ageSec"].as_i64().unwrap_or(0)))
    });

    json!({
        "asOf": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs()).unwrap_or(0),
        "total": FILES.len(),
        "stale": stale,
        "missing": missing,
        "items": items,
        "note": "Поріг життя вказано поряд із кожним набором. Нуль означає «не старіє за призначенням» — довідник або разове дослідження, і позначати його застарілим було б хибою.",
    })
}

pub async fn route(State(s): State<Ctx>) -> impl IntoResponse {
    Json(build(&s.dataset))
}
