//! ВИКЛИК ФУНКЦІЙ: модель сама дістає потрібні дані замість готового контексту.
//!
//! Навіщо: у конвеєрі ~30 жорстких інтентів, і все, що в них не вписалось, отримувало
//! ОДИН fallback-якір про наративи. Нічний прогін показав наслідок: 36 із 70 відповідей
//! нижче 6 балів, і всі слабкі — дослівно однакові. Питання «чому CAMP пастка» і
//! «як виявляєте координовані групи» діставали текст про depin/defi.
//!
//! Тут інакше: модель отримує НАБІР ІНСТРУМЕНТІВ і сама вирішує, що викликати під
//! конкретне питання. Дані по-старому лишаються детермінованими — інструменти читають
//! той самий JSON, тож вигадати число модель не може. Свобода лише у виборі, ЩО дістати.
//!
//! Межа: якщо жоден інструмент не дав даних — чесна відмова, а не переказ чогось іншого.

use serde_json::{json, Value};


/// Тека з даними. Інструменти читають файли напряму — так вони не залежать від
/// життєвого циклу серверного стану й лишаються тестованими.
fn dataset_dir() -> String {
    std::env::var("DATASET_DIR").unwrap_or_else(|_| "/opt/wallet-tracker/dataset".to_string())
}

fn read_json(name: &str) -> Option<Value> {
    let path = format!("{}/{}", dataset_dir(), name);
    let txt = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&txt).ok()
}

/// Обидва файли активів мають форму {"assets": [...]}.
fn read_assets(name: &str) -> Option<Vec<Value>> {
    read_json(name)?["assets"].as_array().cloned()
}


/// Знайти в питанні символ токена, який є в наших даних. Слабка модель часто не
/// здогадується викликати інструмент за символом — тому шукаємо детерміновано.
/// Порівнюємо лише зі СПРАВЖНІМИ символами з датасету: так «АКТИВ» чи «ГРУПА»
/// не будуть прийняті за тікер.
fn detect_symbol(q: &str) -> Option<String> {
    let mut known: Vec<String> = Vec::new();
    for f in ["capital-group-assets.json", "cmc-whale-assets.json"] {
        if let Some(arr) = read_assets(f) {
            for a in arr {
                if let Some(sym) = a["symbol"].as_str() {
                    if sym.len() >= 2 {
                        known.push(sym.to_uppercase());
                    }
                }
            }
        }
    }
    let words: Vec<String> = q
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() >= 2)
        .map(|w| w.to_uppercase())
        .collect();
    // найдовший збіг — щоб «ETH» не перебив «WETH»
    words
        .iter()
        .filter(|w| known.iter().any(|k| k == *w))
        .max_by_key(|w| w.len())
        .cloned()
}


/// Група за ідентифікатором (незалежно від регістру).
fn find_group(id_upper: &str) -> Option<Value> {
    let d = read_json("capital-groups.json")?;
    d["groups"].as_array()?.iter().find(|g| {
        g["id"].as_str().unwrap_or("").to_uppercase() == id_upper
    }).cloned()
}

/// Перелік ідентифікаторів груп — показуємо, коли запитану групу не знайдено.
fn group_ids(limit: usize) -> Vec<String> {
    read_json("capital-groups.json")
        .and_then(|d| d["groups"].as_array().cloned())
        .map(|gs| {
            gs.iter()
                .filter_map(|g| g["id"].as_str().map(|s| s.to_string()))
                .take(limit)
                .collect()
        })
        .unwrap_or_default()
}

/// Ідентифікатор групи, згаданий у питанні (NET, HUB, CG1 …). Порівнюємо зі
/// СПРАВЖНІМИ ідентифікаторами, щоб звичайні слова не приймались за назву групи.
fn detect_group_id(q: &str) -> Option<String> {
    let ids = group_ids(200);
    let words: Vec<String> = q
        .split(|c: char| !(c.is_alphanumeric() || c == '-'))
        .filter(|w| w.len() >= 2)
        .map(|w| w.to_uppercase())
        .collect();
    words
        .iter()
        .filter(|w| ids.iter().any(|i| i.to_uppercase() == **w))
        .max_by_key(|w| w.len())
        .cloned()
}

/// Адреса гаманця у питанні.
fn detect_address(q: &str) -> Option<String> {
    q.split(|c: char| !(c.is_alphanumeric() || c == 'x' || c == 'X'))
        .find(|w| w.len() >= 10 && (w.starts_with("0x") || w.starts_with("0X")))
        .map(|w| w.to_lowercase())
}


/// Значущі числа рядка у нормалізованому вигляді: лише цифри, без ком, пробілів
/// і десяткової крапки. Так «3 303 343.62», «3,303,343.62» і «3303343.62» дають
/// однаковий відбиток, а зсув коми («330,334.36») — інший.
fn digits_of(text: &str) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    let mut cur = String::new();
    for ch in text.chars() {
        if ch.is_ascii_digit() {
            cur.push(ch);
        } else if ch == ',' || ch == '.' || ch == ' ' || ch == '\u{a0}' {
            // роздільники всередині числа не обриваємо
            continue;
        } else {
            if cur.len() >= 4 {
                out.insert(cur.clone());
            }
            cur.clear();
        }
    }
    if cur.len() >= 4 {
        out.insert(cur);
    }
    out
}

/// Чи всі великі числа відповіді присутні у зібраних даних.
fn numbers_are_grounded(answer: &str, facts: &str) -> bool {
    let src = digits_of(facts);
    if src.is_empty() {
        return true; // нема з чим звіряти — не блокуємо
    }
    digits_of(answer).iter().all(|n| src.contains(n))
}


/// Частка кирилиці серед літер. Модель періодично зривається на іншу мову
/// (спостережено китайську), і жодне правило в промпті цього не спиняє.
fn looks_ukrainian(text: &str) -> bool {
    let letters: Vec<char> = text.chars().filter(|c| c.is_alphabetic()).collect();
    if letters.len() < 15 {
        return true; // надто коротко, щоб судити
    }
    let cyr = letters
        .iter()
        .filter(|c| matches!(**c, 'а'..='я' | 'А'..='Я' | 'і' | 'І' | 'ї' | 'Ї' | 'є' | 'Є' | 'ґ' | 'Ґ'))
        .count();
    (cyr as f64) / (letters.len() as f64) >= 0.6
}

/// Відповідь суперечить сама собі: заявляє про брак даних і тут же їх наводить.
/// Це не причіпка до стилю — користувач не може зрозуміти, вірити цифрам чи ні.
fn self_contradictory(text: &str) -> bool {
    let low = text.to_lowercase();
    const DENIALS: &[&str] = &[
        "таких даних у мене немає", "таких даних немає", "не маю даних",
        "бракує інформації", "не можу надати інформацію", "такого поняття в системі немає",
    ];
    let denies = DENIALS.iter().any(|d| low.contains(d));
    if !denies {
        return false;
    }
    // чи є після відмови конкретика: відсотки або великі числа
    let has_facts = low.contains('%')
        || low.contains("відсотк")
        || text
            .split(|c: char| !c.is_ascii_digit())
            .any(|n| n.len() >= 4);
    denies && has_facts
}

/// Чи згадано в питанні ідентифікатор реальної групи капіталу.
pub fn mentions_group(q: &str) -> bool { detect_group_id(q).is_some() }

/// Опис інструментів у форматі OpenAI/Ollama function calling.
pub fn tool_specs() -> Value {
    json!([
        {
            "type": "function",
            "function": {
                "name": "phases_summary",
                "description": "Зведення по фазах китових родин: скільки набирають, роздають, нейтральні, і загальний висновок.",
                "parameters": {"type": "object", "properties": {}, "required": []}
            }
        },
        {
            "type": "function",
            "function": {
                "name": "coins_by_phase",
                "description": "Список монет (символів), які кити зараз накопичують або роздають, із частотами.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "phase": {"type": "string", "enum": ["набирають", "роздають"],
                                  "description": "Яку фазу показати"}
                    },
                    "required": ["phase"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "narrative_flows",
                "description": "Чисті потоки капіталу китів по наративах (DePIN, DeFi, мемкоїни тощо) за місяць і за 3 місяці, зі знаком напряму.",
                "parameters": {"type": "object", "properties": {}, "required": []}
            }
        },
        {
            "type": "function",
            "function": {
                "name": "top_families",
                "description": "Найбільші китові родини: розмір, фаза, архетип, домінантний сектор, топ-токен емісії.",
                "parameters": {
                    "type": "object",
                    "properties": {"limit": {"type": "integer", "description": "скільки родин повернути, 1..10"}},
                    "required": []
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "asset_concentration",
                "description": "Концентрація емісії конкретного активу за його СИМВОЛОМ (напр. LINK, SHIB, CAMP, DOT): яку частку обігу тримають топ-10 і топ-50 холдерів, скільки викупила група капіталу, чи це реальна група чи артефакт.",
                "parameters": {
                    "type": "object",
                    "properties": {"symbol": {"type": "string", "description": "символ токена, напр. LINK"}},
                    "required": ["symbol"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "top_concentrated_assets",
                "description": "Активи з НАЙБІЛЬШОЮ концентрацією емісії — де найбільша частка обігу зосереджена в небагатьох руках. Відповідає на питання «де найбільша концентрація».",
                "parameters": {
                    "type": "object",
                    "properties": {"limit": {"type": "integer", "description": "скільки активів, 1..15"}},
                    "required": []
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "capital_groups",
                "description": "Групи капіталу системи: скільки адрес у групі, реальний і паперовий капітал у доларах. Відповідає на питання про найбільші групи та їхній розмір.",
                "parameters": {
                    "type": "object",
                    "properties": {"limit": {"type": "integer", "description": "скільки груп, 1..15"}},
                    "required": []
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "group_details",
                "description": "Повна картка КОНКРЕТНОЇ групи капіталу за її ідентифікатором (напр. NET, HUB, CG1, NET-WEAK, INFRA): скільки адрес, реальний і паперовий капітал, реалізовна вартість, притік/відтік/нетто коштів, нативний ETH.",
                "parameters": {
                    "type": "object",
                    "properties": {"id": {"type": "string", "description": "ідентифікатор групи, напр. NET або CG1"}},
                    "required": ["id"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "group_members",
                "description": "Учасники КОНКРЕТНОЇ групи капіталу: адреси гаманців і крок віддалення від ядра групи (hop). Використовуй, коли питають про склад групи чи її адреси.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "id": {"type": "string", "description": "ідентифікатор групи"},
                        "limit": {"type": "integer", "description": "скільки адрес показати, 1..25"}
                    },
                    "required": ["id"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "find_group_by_address",
                "description": "Знайти, до якої групи капіталу належить конкретна адреса гаманця (0x...). Використовуй, коли в питанні є адреса.",
                "parameters": {
                    "type": "object",
                    "properties": {"address": {"type": "string", "description": "адреса гаманця 0x..."}},
                    "required": ["address"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "glossary_lookup",
                "description": "Точне визначення терміна системи: частка емісії, реальний капітал, група капіталу, кит, координація, RiskScore тощо.",
                "parameters": {
                    "type": "object",
                    "properties": {"term": {"type": "string", "description": "термін або його частина"}},
                    "required": ["term"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "system_knowledge",
                "description": "Знання про сам продукт: як працює конвеєр, на чому базуються дані, що вже знайдено (кейси zkSync, петля казначейства, пастка CAMP, артефакт UNI), що спростовано, чого система не вміє.",
                "parameters": {
                    "type": "object",
                    "properties": {"topic": {"type": "string", "description": "про що саме: знахідки, обмеження, конвеєр, дані, стан активу"}},
                    "required": ["topic"]
                }
            }
        }
    ])
}

/// Виконати інструмент. Читає з уже побудованого детермінованого контексту.
pub fn execute(name: &str, args: &Value, ctx: &Value) -> Value {
    match name {
        "phases_summary" => json!({
            "висновок": ctx["фазиРодин"]["висновок"].clone(),
            "набирають": ctx["фазиРодин"]["набирають"].clone(),
            "роздають": ctx["фазиРодин"]["роздають"].clone(),
            "нейтральні": ctx["фазиРодин"]["нейтральні"].clone(),
        }),

        "coins_by_phase" => {
            let ph = args["phase"].as_str().unwrap_or("набирають");
            let key = if ph.contains("розд") { "монетиЩоРоздають" } else { "монетиЩоНабирають" };
            json!({ "фаза": ph, "монети": ctx[key].clone() })
        }

        "narrative_flows" => json!({
            "заходять": ctx["наративКудиЗаходятьГрошіКитів"].clone(),
            "виходять": ctx["наративЗвідкиВиходятьГрошіКитів"].clone(),
            "ротація": ctx["ротаціяНаративів"].clone(),
            "підказка": "додатнє нетто = притік, від'ємне = відтік; мітка напряму своя для кожного періоду",
        }),

        "top_families" => {
            let lim = args["limit"].as_u64().unwrap_or(5).clamp(1, 10) as usize;
            let all = ctx["топРодини"].as_array().cloned().unwrap_or_default();
            json!({
                "найбільша": ctx["найбільшаРодина"].clone(),
                "топ": all.into_iter().take(lim).collect::<Vec<_>>(),
            })
        }

        "asset_concentration" => {
            let sym = args["symbol"].as_str().unwrap_or("").to_uppercase();
            if sym.is_empty() {
                return json!({"помилка": "не вказано символ"});
            }
            let mut found = Vec::new();
            for (file, label) in [("capital-group-assets.json", "група капіталу"),
                                  ("cmc-whale-assets.json", "скан китових активів")] {
                if let Some(arr) = read_assets(file) {
                    for a in arr {
                        if a["symbol"].as_str().unwrap_or("").to_uppercase() == sym {
                            found.push(json!({
                                "джерело": label,
                                "символ": a["symbol"].clone(),
                                "назва": a["name"].clone(),
                                "ранг": a["rank"].clone(),
                                "топ10_відсоток_емісії": a["top10pct"].clone(),
                                "топ50_відсоток_емісії": a["top50pct"].clone(),
                                "викуплено_групою": a["groupBought"].clone(),
                                "реальна_група": a["real"].clone(),
                            }));
                        }
                    }
                }
            }
            if found.is_empty() {
                json!({"знайдено": false, "символ": sym,
                       "пояснення": "цього активу немає в даних про концентрацію"})
            } else {
                json!({"знайдено": true, "символ": sym, "дані": found,
                       "підказка": "top10pct — частка обігу в топ-10 холдерів; \
40-60% дає контроль, понад 70% зазвичай означає, що позицію неможливо продати"})
            }
        }

        "top_concentrated_assets" => {
            let lim = args["limit"].as_u64().unwrap_or(10).clamp(1, 15) as usize;
            let mut rows: Vec<(f64, Value)> = Vec::new();
            for file in ["capital-group-assets.json", "cmc-whale-assets.json"] {
                if let Some(arr) = read_assets(file) {
                    for a in arr {
                        let pct = a["top10pct"].as_f64().unwrap_or(0.0);
                        if pct > 0.0 {
                            rows.push((pct, json!({
                                "символ": a["symbol"].clone(),
                                "назва": a["name"].clone(),
                                "топ10_відсоток_емісії": pct,
                                "ранг": a["rank"].clone(),
                            })));
                        }
                    }
                }
            }
            rows.sort_by(|x, y| y.0.partial_cmp(&x.0).unwrap_or(std::cmp::Ordering::Equal));
            rows.dedup_by(|a, b| a.1["символ"] == b.1["символ"]);
            json!({
                "активи": rows.into_iter().take(lim).map(|r| r.1).collect::<Vec<_>>(),
                "підказка": "висока концентрація НЕ означає можливість пампу: \
при тонкій ліквідності позицію неможливо продати"
            })
        }

        "capital_groups" => {
            let lim = args["limit"].as_u64().unwrap_or(8).clamp(1, 15) as usize;
            match read_json("capital-groups.json") {
                Some(d) => {
                    let mut gs: Vec<Value> = d["groups"].as_array().cloned().unwrap_or_default()
                        .into_iter()
                        .map(|g| json!({
                            "id": g["id"].clone(),
                            "адрес": g["members"].as_array().map(|m| m.len()).unwrap_or(0),
                            "реальний_капітал_usd": g["totalUsd"].clone(),
                            "паперовий_капітал_usd": g["totalUsdPaper"].clone(),
                        }))
                        .collect();
                    gs.sort_by(|a, b| {
                        b["реальний_капітал_usd"].as_f64().unwrap_or(0.0)
                            .partial_cmp(&a["реальний_капітал_usd"].as_f64().unwrap_or(0.0))
                            .unwrap_or(std::cmp::Ordering::Equal)
                    });
                    json!({"всього_груп": d["groups"].as_array().map(|a| a.len()).unwrap_or(0),
                           "групи": gs.into_iter().take(lim).collect::<Vec<_>>()})
                }
                None => json!({"знайдено": false, "пояснення": "файл груп капіталу недоступний"}),
            }
        }

        "group_details" => {
            let want = args["id"].as_str().unwrap_or("").to_uppercase();
            match find_group(&want) {
                Some(g) => json!({
                    "id": g["id"].clone(),
                    "назва": g["name"].clone(),
                    "адрес_у_групі": g["size"].clone(),
                    "реальний_капітал_usd": g["totalUsd"].clone(),
                    "паперовий_капітал_usd": g["totalUsdPaper"].clone(),
                    "реалізовна_вартість_usd": g["realizableUsd"].clone(),
                    "нативний_eth_usd": g["nativeEthUsd"].clone(),
                    "притік_usd": g["moneyInUsd"].clone(),
                    "відтік_usd": g["moneyOutUsd"].clone(),
                    "нетто_usd": g["moneyNetUsd"].clone(),
                    "основа_оцінки": g["capitalBasis"].clone(),
                    "підказка": "реальний капітал рахується за ліквідністю, паперовий — за спотовою ціною; \
різниця між ними показує, скільки з позиції насправді неможливо продати"
                }),
                None => json!({
                    "знайдено": false, "запит": want,
                    "доступні_групи": group_ids(20),
                }),
            }
        }

        "group_members" => {
            let want = args["id"].as_str().unwrap_or("").to_uppercase();
            let lim = args["limit"].as_u64().unwrap_or(10).clamp(1, 25) as usize;
            match find_group(&want) {
                Some(g) => {
                    let all = g["members"].as_array().cloned().unwrap_or_default();
                    let total = all.len();
                    let shown: Vec<Value> = all
                        .into_iter()
                        .take(lim)
                        .map(|m| json!({
                            "адреса": m["address"].clone(),
                            "крок_від_ядра": m["hop"].clone(),
                        }))
                        .collect();
                    let declared = g["size"].as_u64().unwrap_or(total as u64) as usize;
                    json!({
                        "група": g["id"].clone(),
                        "усього_адрес": declared,
                        "адрес_у_списку": total,
                        "показано": shown.len(),
                        "учасники": shown,
                        "підказка": "hop — крок віддалення від ядра; null означає ядро. \nЯкщо адрес_у_списку менше за усього_адрес — повний перелік обрізано у датасеті, \nі це обмеження даних, а не помилка."
                    })
                }
                None => json!({
                    "знайдено": false, "запит": want,
                    "доступні_групи": group_ids(20),
                }),
            }
        }

        "find_group_by_address" => {
            let addr = args["address"].as_str().unwrap_or("").to_lowercase();
            if addr.len() < 10 {
                return json!({"помилка": "адреса виглядає некоректною"});
            }
            match read_json("capital-groups.json") {
                Some(d) => {
                    let groups = d["groups"].as_array().cloned().unwrap_or_default();
                    for g in groups {
                        if let Some(ms) = g["members"].as_array() {
                            if let Some(m) = ms.iter().find(|m| {
                                m["address"].as_str().unwrap_or("").to_lowercase() == addr
                            }) {
                                return json!({
                                    "знайдено": true,
                                    "адреса": addr,
                                    "група": g["id"].clone(),
                                    "крок_від_ядра": m["hop"].clone(),
                                    "адрес_у_групі": g["size"].clone(),
                                    "реальний_капітал_групи_usd": g["totalUsd"].clone(),
                                });
                            }
                        }
                    }
                    json!({"знайдено": false, "адреса": addr,
                           "пояснення": "ця адреса не входить у жодну з відомих груп капіталу"})
                }
                None => json!({"знайдено": false, "пояснення": "дані груп недоступні"}),
            }
        }

        "glossary_lookup" => {
            let term = args["term"].as_str().unwrap_or("");
            match crate::glossary::find_term(term) {
                Some(t) => json!({
                    "термін": t.canon, "коротко": t.short,
                    "деталі": t.detail, "джерело": t.source
                }),
                None => json!({
                    "знайдено": false,
                    "доступні_терміни": crate::glossary::known_terms(),
                }),
            }
        }

        "system_knowledge" => {
            let topic = args["topic"].as_str().unwrap_or("");
            match crate::product_kb::find_topic(topic) {
                Some(t) => json!({ "тема": t.canon, "відповідь": t.answer }),
                None => json!({
                    "знайдено": false,
                    "доступні_теми": crate::product_kb::TOPICS.iter()
                        .map(|t| t.canon).collect::<Vec<_>>(),
                }),
            }
        }

        _ => json!({ "помилка": format!("невідомий інструмент: {name}") }),
    }
}

/// Системна інструкція для режиму інструментів. Головне правило — чесна відмова.
pub const SYSTEM: &str = "Ти — AI-аналітик Wallet Radar. Тобі доступні ІНСТРУМЕНТИ для отримання даних.\n\
ПРАВИЛА:\n\
1) Виклич ті інструменти, які потрібні САМЕ для цього питання. Можна кілька.\n\
2) Відповідай ЛИШЕ українською, 2-5 речень, спираючись ВИКЛЮЧНО на дані з інструментів.\n\
3) Жодних вигаданих чисел, символів, адрес чи пояснень. Числа — тільки з інструментів.\n\
4) Якщо інструменти не дали даних для відповіді — скажи ПРЯМО: 'Таких даних у мене немає' \
і коротко назви, чого саме бракує. Не підміняй відповідь іншою темою. \
АЛЕ: якщо у фактах Є цифри по темі питання — ЗАБОРОНЕНО починати з відмови. \
Відмова і наведення даних в одній відповіді — груба помилка.\n\
5) Не прогнозуй ціни. На питання про майбутню ціну відповідай: 'Я не прогнозую ціни — \
я показую лише потоки китів у наявних даних.'\n\
6) Не переказуй усе, що повернув інструмент — бери лише те, що стосується питання.
7) КАТЕГОРИЧНО не пояснюй терміни «зі свого досвіду». Якщо терміна немає ні у фактах, ні в інструментах — скажи Такого поняття в системі немає, і НЕ вигадуй, що воно означає.
8) ЯКЩО у фактах є ДАНІ ПРО АКТИВ або ДАНІ ПРО ГРУПУ — обовязково відповідай на їх основі, навіть коли питання оцінне (чи безпечний, чи вартий уваги, що з ним). Правило 7 стосується НЕВІДОМИХ ТЕРМІНІВ, а не оцінних питань про наявні дані.
9) ЯК ЧИТАТИ КОНЦЕНТРАЦІЮ: до 40 відсотків у топ-10 — розпорошено, ризик контролю низький; 40-60 — група здатна керувати ціною; понад 70 — актив контрольований, і позицію зазвичай НЕМОЖЛИВО продати без обвалу, тобто висока концентрація означає РИЗИК, а не гарантію зростання. Так і відповідай на питання про безпечність активу.";

/// Максимум раундів «модель просить дані → ми віддаємо». Більше не потрібно:
/// на практиці вистачає 1-2, а обмеження боронить від нескінченного циклу.
pub const MAX_ROUNDS: usize = 3;

/// Цикл виклику функцій: модель просить дані → віддаємо → вона формулює відповідь.
/// `None` означає, що режим не дав результату — викликач має перейти на старий шлях
/// або чесно відмовити.
pub async fn run(
    question: &str,
    ctx: &Value,
    base: &str,
    model: &str,
    key: &str,
) -> Option<String> {
    let client = reqwest::Client::new();
    let url = format!("{}/chat/completions", base.trim_end_matches('/'));

    // Слабка 7B-модель часто НЕ здогадується викликати потрібний інструмент і починає
    // фантазувати. Тому релевантні факти шукаємо детерміновано ЗАЗДАЛЕГІДЬ і кладемо
    // в контекст одразу — інструменти лишаються для решти.
    let mut prefetched = String::new();
    if let Some(t) = crate::glossary::find_term(question) {
        prefetched.push_str(&format!(
            "ВИЗНАЧЕННЯ «{}»: {} {}

", t.canon, t.short, t.detail));
    }
    if let Some(sym) = detect_symbol(question) {
        let d = execute("asset_concentration", &json!({"symbol": sym}), ctx);
        if d["знайдено"] == true {
            prefetched.push_str(&format!(
                "ДАНІ ПРО АКТИВ: {}

",
                serde_json::to_string(&d).unwrap_or_default()));
        }
    }
    if let Some(gid) = detect_group_id(question) {
        let d = execute("group_details", &json!({"id": gid}), ctx);
        if d["знайдено"] != false {
            prefetched.push_str(&format!(
                "ДАНІ ПРО ГРУПУ: {}\n\n", serde_json::to_string(&d).unwrap_or_default()));
        }
    }
    if let Some(addr) = detect_address(question) {
        let d = execute("find_group_by_address", &json!({"address": addr}), ctx);
        prefetched.push_str(&format!(
            "ПОШУК АДРЕСИ: {}\n\n", serde_json::to_string(&d).unwrap_or_default()));
    }
    // Питання про НАЙБІЛЬШУ концентрацію: модель не завжди здогадується покликати
    // інструмент, а без фактів фолбек не має на чому спрацювати і питання падає
    // у старий конвеєр. Тому підкладаємо топ детерміновано.
    {
        let ql = question.to_lowercase();
        let about_conc = ql.contains("концентрац") || ql.contains("сконцентр");
        let about_top = ql.contains("найбільш") || ql.contains("де ") || ql.contains("топ")
            || ql.contains("понад") || ql.contains("контролюються");
        if about_conc && about_top {
            let d = execute("top_concentrated_assets", &json!({"limit": 8}), ctx);
            prefetched.push_str(&format!(
                "КОНЦЕНТРАЦІЯ-ТОП: {}

", serde_json::to_string(&d).unwrap_or_default()));
        }
    }
    if let Some(t) = crate::product_kb::find_topic(question) {
        prefetched.push_str(&format!("ЗНАННЯ ПРО СИСТЕМУ — {}:
{}

", t.canon, t.answer));
    }

    let user_msg = if prefetched.is_empty() {
        question.to_string()
    } else {
        format!("ПОПЕРЕДНЬО ЗНАЙДЕНІ ФАКТИ (використай їх, вони точні):
{}
ПИТАННЯ: {}",
                prefetched, question)
    };

    // усе, що інструменти реально повернули — еталон для перевірки чисел
    let mut facts_seen = prefetched.clone();

    let mut messages: Vec<Value> = vec![
        json!({"role": "system", "content": SYSTEM}),
        json!({"role": "user", "content": user_msg}),
    ];

    for _round in 0..MAX_ROUNDS {
        let body = json!({
            "model": model,
            "temperature": 0.0,
            "max_tokens": 700,
            "messages": messages,
            "tools": tool_specs(),
        });

        let resp = client
            .post(&url)
            .header("authorization", format!("Bearer {key}"))
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .ok()?;

        if !resp.status().is_success() {
            return None;
        }
        let v: Value = resp.json().await.ok()?;
        let msg = &v["choices"][0]["message"];

        // Модель попросила дані — виконуємо КОЖЕН виклик і повертаємо результати.
        let calls = msg["tool_calls"].as_array().cloned().unwrap_or_default();
        if !calls.is_empty() {
            messages.push(msg.clone());
            for c in calls {
                let name = c["function"]["name"].as_str().unwrap_or("");
                // аргументи приходять рядком-JSON; порожні/зламані трактуємо як {}
                let args: Value = c["function"]["arguments"]
                    .as_str()
                    .and_then(|s| serde_json::from_str(s).ok())
                    .or_else(|| c["function"]["arguments"].as_object().map(|_| c["function"]["arguments"].clone()))
                    .unwrap_or_else(|| json!({}));

                let result = execute(name, &args, ctx);
                facts_seen.push_str(&serde_json::to_string(&result).unwrap_or_default());
                messages.push(json!({
                    "role": "tool",
                    "tool_call_id": c["id"].clone(),
                    "name": name,
                    "content": serde_json::to_string(&result).unwrap_or_else(|_| "{}".into()),
                }));
            }
            continue; // наступний раунд: модель бачить дані
        }

        // Інструментів більше не просить — це фінальна відповідь.
        let text = msg["content"].as_str().unwrap_or("").trim().to_string();
        if text.is_empty() {
            return None;
        }
        // Модель могла зіпсувати число при переказі (реальний випадок: зсув коми
        // на порядок). Правдоподібна, але неіснуюча цифра гірша за відсутність
        // відповіді — тож відхиляємо й пускаємо питання далі по ланцюгу.
        // Мова, самосуперечність і числа — три способи, якими 7B-модель псує
        // правильні дані. Реакція однакова: віддати факти без переказу.
        if !looks_ukrainian(&text) || self_contradictory(&text) || !numbers_are_grounded(&text, &facts_seen) {
            // Модель зіпсувала цифру. Не падаємо в загальний конвеєр (він дасть
            // шаблонну відповідь не по темі) — віддаємо СИРІ дані інструментів:
            // вони точні за побудовою, хай і сухіші за формулюванням.
            if !facts_seen.trim().is_empty() {
                return Some(format!(
                    "Наводжу дані без переказу, щоб не спотворити цифри:

{}",
                    facts_seen.trim()
                ));
            }
            return None;
        }
        return Some(text);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_ctx() -> Value {
        json!({
            "фазиРодин": {"висновок": "більше набирають", "набирають": 7, "роздають": 3, "нейтральні": 1},
            "монетиЩоНабирають": [{"символ": "LINK", "кількістьРодин": 4}],
            "монетиЩоРоздають": [{"символ": "SHIB", "кількістьРодин": 2}],
            "наративКудиЗаходятьГрошіКитів": "depin",
            "наративЗвідкиВиходятьГрошіКитів": "defi",
            "ротаціяНаративів": {"depin": {"останнійМісяцьНетто": 100.0}},
            "топРодини": [{"розмір": 50}, {"розмір": 30}, {"розмір": 10}],
            "найбільшаРодина": {"розмір": 50}
        })
    }

    #[test]
    fn specs_are_valid_json_array() {
        let s = tool_specs();
        assert!(s.is_array());
        assert_eq!(s.as_array().unwrap().len(), 12);
    }

    #[test]
    fn phases_and_coins_read_from_context() {
        let c = fake_ctx();
        let p = execute("phases_summary", &json!({}), &c);
        assert_eq!(p["набирають"], 7);
        let coins = execute("coins_by_phase", &json!({"phase": "роздають"}), &c);
        assert_eq!(coins["монети"][0]["символ"], "SHIB");
    }

    #[test]
    fn top_families_respects_limit() {
        let c = fake_ctx();
        let t = execute("top_families", &json!({"limit": 2}), &c);
        assert_eq!(t["топ"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn glossary_and_kb_are_reachable() {
        let c = fake_ctx();
        let g = execute("glossary_lookup", &json!({"term": "частка емісії"}), &c);
        assert_eq!(g["термін"], "Частка емісії під контролем");
        let k = execute("system_knowledge", &json!({"topic": "що знайшли"}), &c);
        assert_eq!(k["тема"], "Що вже знайдено");
    }

    #[test]
    fn unknown_term_lists_options_instead_of_inventing() {
        let c = fake_ctx();
        let g = execute("glossary_lookup", &json!({"term": "дрейф"}), &c);
        assert_eq!(g["знайдено"], false);
        assert!(g["доступні_терміни"].as_array().unwrap().len() > 5);
    }
}
