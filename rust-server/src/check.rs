//! `/api/check` — перевірка за 30 секунд перед транзакцією.
//!
//! Питання, на яке відповідає цей модуль: людина бачить адресу токена або гаманця
//! УПЕРШЕ і має за півхвилини вирішити, чи мати з нею справу. Усі складові вже є в
//! системі — концентрація, групи капіталу, мітки сутностей, реальна ліквідність —
//! але розкидані по різних маршрутах, і зібрати з них рішення користувач мусив сам.
//!
//! ЧОМУ БЕЗ МЕРЕЖІ. Обіцянка «30 секунд» несумісна з живими запитами: `/api/concentration`
//! на холодному токені робить десятки звернень до Blockscout і йде 12–34 секунди. Тому
//! тут — лише те, що вже лежить у `dataset/`. Відповідь миттєва, і її можна давати
//! програмному агенту, а не лише людині.
//!
//! ГОЛОВНЕ ПРАВИЛО: **відсутність даних — це не «чисто».**
//! Найлегший спосіб зробити такий сервіс шкідливим — повернути «А» на адресу, про яку
//! ми просто нічого не знаємо. Тоді порожнеча читається як схвалення. Тому невідоме
//! отримує окремий стан `?` і `coverage.known = false`, і жодна арифметика не може
//! перетворити його на оцінку. Це та сама помилка, що в цьому проєкті вже коштувала
//! хибних висновків, коли тихий нуль видавали за виміряне значення.

use serde_json::{json, Value};

/// Що саме перевіряємо.
#[derive(PartialEq, Clone, Copy)]
pub enum Kind {
    Token,
    Wallet,
    Unknown,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Kind::Token => "token",
            Kind::Wallet => "wallet",
            Kind::Unknown => "unknown",
        }
    }
}

/// Сигнали, зібрані маршрутом із локальних наборів. Усі поля — `Option`, бо
/// відсутність кожного окремого джерела має бути видно у відповіді, а не мовчки
/// підмінятися нулем.
#[derive(Default)]
pub struct Signals {
    /// Мітка відомої сутності (біржа, інфраструктура, міст) — з `labels`.
    pub label: Option<String>,
    pub label_is_cex: bool,
    pub label_is_infra: bool,

    // --- токен ---
    /// Символ із `token-supply.json`.
    pub symbol: Option<String>,
    /// Частка топ-10 холдерів, %.
    pub top10pct: Option<f64>,
    /// Частка, яку викупила група капіталу, %.
    pub group_bought: Option<f64>,
    /// Чи стоїть за оцінкою РЕАЛЬНА ліквідність пулів (false = лише паперова).
    pub real_liquidity: Option<bool>,

    // --- гаманець ---
    /// Група капіталу, до якої належить адреса.
    pub group_id: Option<String>,
    pub group_name: Option<String>,
    pub group_size: Option<u64>,
    /// Реальний капітал групи, USD.
    pub group_total_usd: Option<f64>,
    /// Сальдо групи: додатне — накопичує, від'ємне — виводить.
    pub group_money_net_usd: Option<f64>,
}

/// Один аргумент вердикту: людський текст + машинна пара «ключ-значення».
fn fact(key: &str, text: String, value: Value) -> Value {
    json!({ "key": key, "text": text, "value": value })
}

fn fmt_usd(v: f64) -> String {
    let a = v.abs();
    if a >= 1e9 {
        format!("{:.2} млрд $", v / 1e9)
    } else if a >= 1e6 {
        format!("{:.1} млн $", v / 1e6)
    } else if a >= 1e3 {
        format!("{:.0} тис. $", v / 1e3)
    } else {
        format!("{v:.0} $")
    }
}

/// Бал 0–100 → літера. Межі навмисно грубі: літера має відповідати на питання
/// «мати справу чи ні», а не вдавати точність, якої в даних немає.
fn grade_of(score: u32) -> &'static str {
    match score {
        85..=100 => "A",
        70..=84 => "B",
        50..=69 => "C",
        30..=49 => "D",
        _ => "F",
    }
}

/// Складає відповідь. Чиста функція: жодного вводу-виводу, тому поведінку можна
/// перевірити тестом, а не тільки очима на живому сервері.
pub fn assess(query: &str, kind: Kind, s: &Signals) -> Value {
    let mut facts: Vec<Value> = vec![];
    let mut flags: Vec<Value> = vec![];
    let mut sources: Vec<&str> = vec![];
    let mut missing: Vec<&str> = vec![];

    // ── Відома сутність. Це не «ризик», це інша природа адреси: біржу чи міст
    //    безглуздо оцінювати тими самими мірками, що приватний гаманець.
    if let Some(name) = &s.label {
        sources.push("labels");
        let what = if s.label_is_cex {
            "біржа"
        } else if s.label_is_infra {
            "інфраструктура"
        } else {
            "відома сутність"
        };
        facts.push(fact(
            "entity",
            format!("Адреса має мітку: {name} ({what})"),
            json!({ "name": name, "cex": s.label_is_cex, "infra": s.label_is_infra }),
        ));
        if s.label_is_cex || s.label_is_infra {
            return json!({
                "query": query,
                "kind": kind.as_str(),
                "grade": "—",
                "score": Value::Null,
                "verdict": format!("Це {what}, а не звичайний контрагент. Оцінювати ризик за холдерами тут немає сенсу — адреса технічна."),
                "facts": facts,
                "flags": flags,
                "coverage": { "known": true, "sources": sources, "missing": missing },
            });
        }
    } else {
        missing.push("labels");
    }

    // ── Оцінка. Починаємо з нейтральних 70 і рухаємось від фактів. Стартове
    //    значення НЕ 100: «нічого поганого не знайдено» не дорівнює «добре».
    let mut score: i32 = 70;
    let mut known = false;

    match kind {
        Kind::Token => {
            if let Some(sym) = &s.symbol {
                known = true;
                sources.push("token-supply");
                facts.push(fact("symbol", format!("Токен {sym}"), json!(sym)));
            } else {
                missing.push("token-supply");
            }

            match s.top10pct {
                Some(top) => {
                    known = true;
                    sources.push("capital-group-assets");
                    facts.push(fact(
                        "concentration",
                        format!("Топ-10 холдерів тримають {top:.1}% обігу"),
                        json!(top),
                    ));
                    // Концентрація — головний важіль: саме вона визначає, чи може
                    // одна сторона вирішити долю ціни без вашої участі.
                    score -= match top {
                        t if t >= 90.0 => 60,
                        t if t >= 70.0 => 40,
                        t if t >= 50.0 => 25,
                        t if t >= 30.0 => 10,
                        _ => 0,
                    };
                    if top >= 70.0 {
                        flags.push(json!({
                            "key": "concentration",
                            "text": format!("Обіг сконцентрований: топ-10 контролюють {top:.1}%"),
                        }));
                    }
                }
                None => missing.push("concentration"),
            }

            if let Some(gb) = s.group_bought {
                if gb > 0.0 {
                    known = true;
                    facts.push(fact(
                        "group_bought",
                        format!("Відома група капіталу викупила {gb:.1}% обігу"),
                        json!(gb),
                    ));
                    if gb >= 50.0 {
                        score -= 15;
                        flags.push(json!({
                            "key": "group_control",
                            "text": format!("Понад половину обігу ({gb:.1}%) тримає одна відома група"),
                        }));
                    }
                }
            }

            // Паперова оцінка без реальної ліквідності — окремий, дуже практичний
            // ризик: вийти з активу може бути нікуди, навіть якщо «капіталізація» велика.
            if s.real_liquidity == Some(false) {
                known = true;
                score -= 20;
                flags.push(json!({
                    "key": "no_real_liquidity",
                    "text": "За оцінкою немає реальної ліквідності пулів — вийти з активу може бути нікуди",
                }));
            }
        }

        Kind::Wallet => {
            match (&s.group_id, &s.group_name) {
                (Some(id), name) => {
                    known = true;
                    sources.push("capital-groups");
                    let nm = name.clone().unwrap_or_default();
                    let size = s.group_size.unwrap_or(0);
                    facts.push(fact(
                        "group",
                        format!("Належить до групи капіталу {id} ({nm}), у ній {size} гаманців"),
                        json!({ "id": id, "name": nm, "size": size }),
                    ));

                    if let Some(t) = s.group_total_usd {
                        facts.push(fact(
                            "group_capital",
                            format!("Реальний капітал групи — {}", fmt_usd(t)),
                            json!(t),
                        ));
                    }

                    // Сальдо групи: чи вона зараз набирає, чи роздає. Роздача — це
                    // саме той момент, коли на іншому боці угоди опиняєтесь ви.
                    if let Some(net) = s.group_money_net_usd {
                        let dir = if net < 0.0 { "виводить" } else { "накопичує" };
                        facts.push(fact(
                            "group_flow",
                            format!("Група {dir} кошти: сальдо {}", fmt_usd(net)),
                            json!(net),
                        ));
                        if net < 0.0 {
                            score -= 20;
                            flags.push(json!({
                                "key": "group_distributing",
                                "text": "Група зараз виводить кошти — імовірна фаза розподілу",
                            }));
                        }
                    }

                    // Належність до великої координованої групи сама по собі —
                    // підстава бути обережним, незалежно від напрямку потоку.
                    if size >= 100 {
                        score -= 15;
                        flags.push(json!({
                            "key": "large_group",
                            "text": format!("Адреса — частина великої координованої групи ({size} гаманців)"),
                        }));
                    }
                }
                _ => missing.push("capital-groups"),
            }
        }

        Kind::Unknown => {}
    }

    // ── Невідоме лишається невідомим. Жодна арифметика вище не має права
    //    перетворити порожнечу на оцінку — інакше «нічого не знайшли» читається
    //    як «перевірено, чисто», і сервіс починає шкодити.
    if !known {
        return json!({
            "query": query,
            "kind": kind.as_str(),
            "grade": "?",
            "score": Value::Null,
            "verdict": "Про цю адресу в системі немає даних. Це НЕ означає, що вона безпечна — означає, що ми її не перевіряли.",
            "facts": facts,
            "flags": flags,
            "coverage": { "known": false, "sources": sources, "missing": missing },
        });
    }

    let score = score.clamp(0, 100) as u32;
    let grade = grade_of(score);
    let verdict = match grade {
        "A" => "Тривожних ознак не знайдено серед тих, що ми вміємо перевіряти.",
        "B" => "Переважно чисто, але є на що зважити — дивись факти нижче.",
        "C" => "Є помітні ризики. Варто розібратися до того, як заходити.",
        "D" => "Серйозні ризики. Без окремого розбору краще не заходити.",
        _ => "Знайдено ознаки, за яких втрата коштів імовірна. Не заходити.",
    };

    json!({
        "query": query,
        "kind": kind.as_str(),
        "grade": grade,
        "score": score,
        "verdict": verdict,
        "facts": facts,
        "flags": flags,
        "coverage": { "known": true, "sources": sources, "missing": missing },
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Маршрут: збирання сигналів із локальних наборів даних.
// ─────────────────────────────────────────────────────────────────────────────

use crate::state::{bad_addr, valid_addr, Ctx};
use axum::{extract::{Query, State}, response::IntoResponse, Json};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

#[derive(Deserialize)]
pub struct CheckQuery {
    q: Option<String>,
}

/// Коротка довідка про групу — рівно те, що потрібно для вердикту.
#[derive(Clone)]
struct GroupRef {
    id: String,
    name: String,
    size: u64,
    total_usd: f64,
    money_net_usd: f64,
}

/// Індекс «адреса → група». Будується один раз і перебудовується лише коли
/// `capital-groups.json` змінився на диску: розбирати 1.2 МБ і 127 тис. адрес на
/// кожен запит — це вбити обіцянку миттєвої відповіді. Ключ актуальності — mtime,
/// а не таймер: файл переписують воркери, і саме запис має скидати кеш.
static GROUP_IDX: OnceLock<RwLock<Option<(std::time::SystemTime, HashMap<String, GroupRef>)>>> =
    OnceLock::new();

fn group_of(s: &Ctx, addr_lc: &str) -> Option<GroupRef> {
    let path = s.dataset.root().join("capital-groups.json");
    let mtime = std::fs::metadata(&path).ok()?.modified().ok()?;

    let cell = GROUP_IDX.get_or_init(|| RwLock::new(None));
    if let Ok(g) = cell.read() {
        if let Some((cached, map)) = g.as_ref() {
            if *cached == mtime {
                return map.get(addr_lc).cloned();
            }
        }
    }

    // Кеш застарів або порожній — перебудовуємо.
    let v = s.dataset.read("capital-groups.json")?;
    let groups = v["groups"].as_array()?;
    let mut map: HashMap<String, GroupRef> = HashMap::new();
    for g in groups {
        let r = GroupRef {
            id: g["id"].as_str().unwrap_or_default().to_string(),
            name: g["name"].as_str().unwrap_or_default().to_string(),
            size: g["size"].as_u64().unwrap_or(0),
            total_usd: g["totalUsd"].as_f64().unwrap_or(0.0),
            money_net_usd: g["moneyNetUsd"].as_f64().unwrap_or(0.0),
        };
        if let Some(members) = g["members"].as_array() {
            for m in members {
                if let Some(a) = m["address"].as_str() {
                    map.insert(a.to_ascii_lowercase(), r.clone());
                }
            }
        }
    }
    let out = map.get(addr_lc).cloned();
    if let Ok(mut w) = cell.write() {
        *w = Some((mtime, map));
    }
    out
}

/// `GET /api/check?q=0x…` — одна відповідь про токен або гаманець.
///
/// Свідомо НЕ ходить у мережу: усе береться з `dataset/`. Відповідь однакова для
/// людини й для програмного агента — той самий JSON, стабільні ключі.
pub async fn check_route(State(s): State<Ctx>, Query(q): Query<CheckQuery>) -> impl IntoResponse {
    let raw = q.q.unwrap_or_default();
    let addr = raw.trim().to_string();
    if !valid_addr(&addr) {
        return bad_addr().into_response();
    }
    let lc = addr.to_ascii_lowercase();

    let mut sig = Signals::default();

    // Мітка відомої сутності.
    if let Some(name) = s.labels.label_name(&lc) {
        sig.label_is_cex = s.cex_label.is_match(&name);
        sig.label_is_infra = s.labels.is_infra_addr(&lc);
        sig.label = Some(name);
    }

    // Токен? Перевіряємо за реєстром обігу.
    let supply = s.dataset.read("token-supply.json").unwrap_or(Value::Null);
    let is_token = supply.get(&lc).is_some();
    if is_token {
        sig.symbol = supply[&lc]["symbol"].as_str().map(|x| x.to_string());
    }

    // Дані про актив із набору активів груп капіталу.
    if let Some(av) = s.dataset.read("capital-group-assets.json") {
        if let Some(list) = av["assets"].as_array() {
            if let Some(a) = list.iter().find(|a| {
                a["contract"].as_str().map(|c| c.eq_ignore_ascii_case(&lc)).unwrap_or(false)
            }) {
                sig.top10pct = a["top10pct"].as_f64();
                sig.group_bought = a["groupBought"].as_f64();
                sig.real_liquidity = a["real"].as_bool();
                if sig.symbol.is_none() {
                    sig.symbol = a["symbol"].as_str().map(|x| x.to_string());
                }
            }
        }
    }

    // Гаманець? Належність до групи капіталу.
    if let Some(g) = group_of(&s, &lc) {
        sig.group_id = Some(g.id);
        sig.group_name = Some(g.name);
        sig.group_size = Some(g.size);
        sig.group_total_usd = Some(g.total_usd);
        sig.group_money_net_usd = Some(g.money_net_usd);
    }

    // Тип визначаємо за знайденим, а не за здогадом про формат адреси: контракт і
    // EOA виглядають однаково, і «вгадування» тут дало б впевнений хибний вердикт.
    let kind = if is_token || sig.top10pct.is_some() {
        Kind::Token
    } else if sig.group_id.is_some() || sig.label.is_some() {
        Kind::Wallet
    } else {
        Kind::Unknown
    };

    Json(assess(&addr, kind, &sig)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Найважливіший тест у модулі: порожнеча НЕ перетворюється на «А».
    /// Саме ця помилка робить перевірку шкідливою — користувач читає високу
    /// оцінку як «перевірено», хоча ми просто нічого не знайшли.
    #[test]
    fn unknown_never_becomes_a_grade() {
        let out = assess("0xabc", Kind::Wallet, &Signals::default());
        assert_eq!(out["grade"], "?");
        assert_eq!(out["score"], Value::Null);
        assert_eq!(out["coverage"]["known"], false);
    }

    /// Біржа не отримує оцінку ризику — у неї інша природа.
    #[test]
    fn exchange_is_not_scored() {
        let s = Signals { label: Some("Binance 14".into()), label_is_cex: true, ..Default::default() };
        let out = assess("0xdef", Kind::Wallet, &s);
        assert_eq!(out["grade"], "—");
        assert_eq!(out["coverage"]["known"], true);
    }

    /// Концентрація 96.8% має давати найгіршу літеру, а не «десь посередині».
    #[test]
    fn extreme_concentration_is_f() {
        let s = Signals { symbol: Some("MCDON".into()), top10pct: Some(96.8), ..Default::default() };
        let out = assess("0x4c8", Kind::Token, &s);
        assert_eq!(out["grade"], "F");
        assert!(!out["flags"].as_array().unwrap().is_empty());
    }

    /// Розсіяний токен без інших ознак — висока літера, але не автоматично 100.
    #[test]
    fn dispersed_token_scores_well() {
        let s = Signals { symbol: Some("LINK".into()), top10pct: Some(12.0), ..Default::default() };
        let out = assess("0x514", Kind::Token, &s);
        assert_eq!(out["grade"], "B");
        assert_eq!(out["score"], 70);
    }

    /// Група, що виводить кошти, знижує оцінку і піднімає прапорець.
    #[test]
    fn distributing_group_lowers_grade() {
        let s = Signals {
            group_id: Some("NET".into()),
            group_name: Some("ядро".into()),
            group_size: Some(377),
            group_money_net_usd: Some(-1.5e8),
            ..Default::default()
        };
        let out = assess("0x111", Kind::Wallet, &s);
        assert_eq!(out["score"], 35);
        assert_eq!(out["grade"], "D");
        let keys: Vec<String> = out["flags"].as_array().unwrap().iter()
            .map(|f| f["key"].as_str().unwrap_or_default().to_string()).collect();
        assert!(keys.contains(&"group_distributing".to_string()));
        assert!(keys.contains(&"large_group".to_string()));
    }
}
