//! ДЕТЕРМІНОВАНІ відповіді на найчастіші питання про дані — без виклику моделі.
//!
//! Навіщо: аналіз прогонів показав, що саме ці питання стабільно падали в старий
//! шаблонний конвеєр і псувалися. Модель на них не потрібна взагалі: відповідь
//! повністю визначається кількома полями контексту, і її можна зібрати з фактів.
//!
//! Що це дає:
//!   • швидкість — мілісекунди замість 6-12 секунд
//!   • нуль спотворень — числа беруться з JSON і не проходять через переказ
//!   • стабільність — той самий вхід завжди дає той самий текст
//!
//! Межа застосування: тільки питання з ОДНОЗНАЧНОЮ відповіддю з контексту.
//! Усе, що потребує комбінування чи міркування, лишається моделі з інструментами.

use serde_json::Value;

/// Число з розділювачами тисяч: 1234567.8 → «1 234 568».
fn num(v: &Value) -> String {
    let x = v.as_f64().unwrap_or(0.0);
    let neg = x < 0.0;
    let s = format!("{:.0}", x.abs());
    let mut out = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push('\u{202f}');
        }
        out.push(c);
    }
    let body: String = out.chars().rev().collect();
    if neg { format!("−{body}") } else { body }
}

/// Символи монет із поля контексту, з частотами.
fn coins(ctx: &Value, key: &str, limit: usize) -> Vec<String> {
    ctx[key]
        .as_array()
        .map(|a| {
            a.iter()
                .take(limit)
                .filter_map(|c| {
                    let sym = c["токен"].as_str()?;
                    let n = c["кількістьРодин"].as_u64().unwrap_or(0);
                    Some(if n > 0 {
                        format!("{sym} ({n})")
                    } else {
                        sym.to_string()
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Головний вхід. `None` → питання не з цього класу, хай іде далі.
pub fn try_answer(question: &str, ctx: &Value) -> Option<String> {
    let q = question.to_lowercase();

    // ── монети у фазі накопичення ───────────────────────────────────────────
    let about_coins = q.contains("монет") || q.contains("токен") || q.contains("активи");
    let acc = q.contains("набира") || q.contains("накопич") || q.contains("купу");
    let dist = q.contains("розда") || q.contains("скида") || q.contains("прода");

    if about_coins && acc && !dist {
        let list = coins(ctx, "монетиЩоНабирають", 8);
        if list.is_empty() {
            return Some("Зараз немає монет із вираженим накопиченням у китових родинах.".into());
        }
        return Some(format!(
            "Кити зараз накопичують: {}.\n\nУ дужках — скільки родин набирають цю монету. \
Стейбли й обгортки прибрано: вони не несуть сигналу напрямку. \
Важливо: за нашими даними інтенсивне накопичення частіше передує падінню ціни, \
ніж зростанню — сигнал читається як привід до обережності.",
            list.join(", ")
        ));
    }

    if about_coins && dist && !acc {
        let list = coins(ctx, "монетиЩоРоздають", 8);
        if list.is_empty() {
            return Some("Зараз немає монет із вираженою роздачею у китових родинах.".into());
        }
        return Some(format!(
            "Кити зараз роздають: {}.\n\nУ дужках — скільки родин роздають цю монету. \
Роздача означає, що група продає накопичене; за драбиною виходу це починається \
після 15% вийшлої емісії.",
            list.join(", ")
        ));
    }

    // ── потоки по наративах ─────────────────────────────────────────────────
    let about_flow = q.contains("течуть") || q.contains("потік") || q.contains("заходять")
        || q.contains("виходять") || q.contains("ротаці") || q.contains("наратив");
    if about_flow {
        let inflow = ctx["наративКудиЗаходятьГрошіКитів"].as_str().unwrap_or("—");
        let outflow = ctx["наративЗвідкиВиходятьГрошіКитів"].as_str().unwrap_or("—");
        let mut rows: Vec<String> = Vec::new();
        if let Some(obj) = ctx["ротаціяНаративів"].as_object() {
            for (name, v) in obj.iter() {
                if name == "other" {
                    continue;
                }
                let m = v["останнійМісяцьНетто"].as_f64().unwrap_or(0.0);
                let q3 = v["останні3місяціНетто"].as_f64().unwrap_or(0.0);
                let d1 = if m > 0.0 { "притік" } else if m < 0.0 { "відтік" } else { "нейтрально" };
                let d3 = if q3 > 0.0 { "притік" } else if q3 < 0.0 { "відтік" } else { "нейтрально" };
                rows.push(format!(
                    "{name}: місяць {} $ ({d1}), три місяці {} $ ({d3})",
                    num(&v["останнійМісяцьНетто"]), num(&v["останні3місяціНетто"])));
            }
        }
        rows.sort();
        return Some(format!(
            "Гроші китів зараз заходять у наратив {inflow}, а виходять з {outflow}.\n\n{}\n\n\
Додатнє число означає притік, від'ємне — відтік. Нерозпізнану категорію не враховано: \
туди потрапляють стейбли й невідомі активи, а не справжня тема.",
            if rows.is_empty() { "Деталізації по наративах зараз немає.".to_string() } else { rows.join("\n") }
        ));
    }

    // ── зведення по фазах ───────────────────────────────────────────────────
    // «що зараз на ринку» не містить ані «кити», ані «родини» — але це саме
    // питання про загальний стан, і відповідь на нього однозначна з контексту.
    let market_state = q.contains("на ринку") || q.contains("стан ринку")
        || q.contains("що зараз відбува") || q.contains("загальна картина")
        || q.contains("ситуація на");
    let about_phase = market_state
        || ((q.contains("кити") || q.contains("родин"))
            && (q.contains("купують") || q.contains("продають") || q.contains("фаз")
                || q.contains("що зараз") || q.contains("що відбува")));
    if about_phase {
        let ph = &ctx["фазиРодин"];
        let raw = ph["висновок"].as_str().unwrap_or("невизначено");
        // поле вже містить готове речення — не дублюємо підмет і крапку
        let concl = if raw.ends_with(char::is_alphabetic) { format!("{raw}.") } else { raw.to_string() };
        return Some(format!(
            "Загальний стан: {concl}\n\n\
Набирають — {} родин, роздають — {}, нейтральні — {}.\n\n\
«Набирає» означає накопичення, «роздає» — продаж накопиченого.",
            ph["набирають"], ph["роздають"], ph["нейтральні"]
        ));
    }

    // ── найбільша родина ────────────────────────────────────────────────────
    if q.contains("найбільша родина") || q.contains("найбільшої родини") {
        let f = &ctx["найбільшаРодина"];
        if f.is_null() {
            return None;
        }
        return Some(format!(
            "Найбільша китова родина — {} гаманців.\n\nФаза: {}. Домінантний сектор: {}.\n\n\
Родина — це кластер за схожістю поведінки, а не за прямими переказами. \
Групи капіталу — інша сутність, вони будуються за фактичними зв'язками.",
            f["розмірГаманців"],
            f["фаза"].as_str().unwrap_or("невідомо"),
            f["домінантнийСектор"].as_str().unwrap_or("невизначено"),
        ));
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ctx() -> Value {
        json!({
            "фазиРодин": {"висновок": "більше набирають", "набирають": 7, "роздають": 3, "нейтральні": 1},
            "монетиЩоНабирають": [{"токен": "LINK", "кількістьРодин": 4},
                                  {"токен": "SHIB", "кількістьРодин": 2}],
            "монетиЩоРоздають": [{"токен": "HEX", "кількістьРодин": 3}],
            "наративКудиЗаходятьГрошіКитів": "depin",
            "наративЗвідкиВиходятьГрошіКитів": "defi",
            "ротаціяНаративів": {"depin": {"останнійМісяцьНетто": 749000.0},
                                 "defi": {"останнійМісяцьНетто": -10110.0},
                                 "other": {"останнійМісяцьНетто": 5.0}},
            "найбільшаРодина": {"розмірГаманців": 4602, "фаза": "роздає", "домінантнийСектор": "defi"}
        })
    }

    #[test]
    fn formats_large_numbers_with_separators() {
        assert_eq!(num(&json!(1234567.8)), "1\u{202f}234\u{202f}568");
        assert_eq!(num(&json!(-10110.0)), "−10\u{202f}110");
    }

    #[test]
    fn answers_accumulation_and_distribution_separately() {
        let a = try_answer("які монети зараз набирають кити", &ctx()).unwrap();
        assert!(a.contains("LINK (4)"));
        assert!(!a.contains("HEX"));
        let d = try_answer("які монети роздають", &ctx()).unwrap();
        assert!(d.contains("HEX (3)"));
        assert!(!d.contains("LINK"));
    }

    #[test]
    fn narrative_flows_skip_other_and_mark_direction() {
        let a = try_answer("куди течуть гроші китів", &ctx()).unwrap();
        assert!(a.contains("depin"));
        assert!(a.contains("відтік"));
        assert!(!a.contains("other"));
    }

    #[test]
    fn largest_family_reports_size() {
        let a = try_answer("яка найбільша родина", &ctx()).unwrap();
        assert!(a.contains("4602"));
    }

    #[test]
    fn ignores_questions_outside_its_scope() {
        assert!(try_answer("що таке частка емісії", &ctx()).is_none());
        assert!(try_answer("розкажи про групу NET", &ctx()).is_none());
    }
}
