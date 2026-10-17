//! safewrite — запис результату, який не може знищити попередній.
//!
//! ЩО СТАЛОСЯ 22.08.2026. Групи капіталу перегенерували за новою схемою ідентифікаторів
//! (`CG*` замість `CAP:*`), а чотири біни `rg*` лишились із фільтром `id.starts_with("CAP:")`.
//! Вони перестали знаходити бодай одну групу — і записали цю порожнечу поверх хороших
//! файлів: `rgphase.json`, `rgflow.json`, `rgfund.json`, `rgsync.json` стали 0 байт.
//! Скрипт відзвітував «done», cron-лог лишився чистим. Те саме сталося з
//! `token-prices.json` 23.08 о 01:15 — рівно в час свого cron.
//!
//! Причина одна на всі випадки: `File::create` ОБРІЗАЄ файл одразу, ще до того, як
//! стане відомо, чи є що записувати. Порожній результат і зниклий результат виглядають
//! на диску однаково.
//!
//! ДРУГА ПРИЧИНА, та сама вразливість. Хост вимикається обривом живлення (324 unsafe
//! shutdown у SMART). Обрив посеред `write_all` лишає обрізаний файл — і знову
//! неможливо відрізнити його від чесно порожнього результату.
//!
//! Обидві лікуються одним:
//!   1. РЕЗУЛЬТАТ ПЕРЕВІРЯЄТЬСЯ ДО ЗАПИСУ. Порожній або різко змілілий результат не
//!      затирає попередній — бін падає з поясненням, а дані лишаються.
//!   2. ЗАПИС АТОМАРНИЙ. Спершу `.tmp`, потім `rename`. Обрив живлення лишає або старий
//!      файл цілим, або новий цілим; обрізаного не буває ніколи.
//!
//! Env:
//!   SHRINK_GUARD=5      у скільки разів дозволено змілити файл (0 = вимкнути перевірку)
//!   ALLOW_EMPTY_WRITE=1 свідомо дозволити порожній запис (коли дані справді зникли)

use std::io::Write;

/// Чому запис не відбувся. Викликач має вирішити: впасти чи попередити — але мовчки
/// продовжити не може, бо саме мовчання й коштувало чотирьох датасетів.
#[derive(Debug)]
pub enum Refused {
    /// Результат порожній, а на диску лежить непорожній файл.
    WouldEmpty { existing: u64 },
    /// Результат різко змілів проти того, що вже є.
    WouldShrink { existing: u64, new: u64, times: f64 },
    /// Не вдалося записати (диск, права).
    Io(std::io::Error),
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refused::WouldEmpty { existing } => write!(f,
                "результат ПОРОЖНІЙ, а на диску {existing} байт — запис скасовано, старі дані збережено. \
                 Якщо порожнеча очікувана: ALLOW_EMPTY_WRITE=1"),
            Refused::WouldShrink { existing, new, times } => write!(f,
                "результат змілів у {times:.1}× ({existing} → {new} байт) — запис скасовано. \
                 Якщо це очікувано: SHRINK_GUARD=0"),
            Refused::Io(e) => write!(f, "запис не вдався: {e}"),
        }
    }
}

fn env_num<T: std::str::FromStr>(k: &str, d: T) -> T {
    std::env::var(k).ok().and_then(|s| s.parse().ok()).unwrap_or(d)
}

/// Записує текст у файл: спершу перевіряє, потім атомарно замінює.
///
/// Повертає `Ok(())` лише коли новий вміст справді ліг на місце.
pub fn save_str(path: &str, body: &str) -> Result<(), Refused> {
    let new_len = body.len() as u64;
    let existing = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let allow_empty = std::env::var("ALLOW_EMPTY_WRITE").map(|v| v == "1").unwrap_or(false);
    let guard: f64 = env_num("SHRINK_GUARD", 5.0f64);

    // «Порожній» — це не лише 0 байт: `{}` і `[]` теж означають «нічого не знайшлось».
    let trimmed = body.trim();
    let empty_result = trimmed.is_empty()
        || trimmed == "{}"
        || trimmed == "[]"
        || trimmed == "null";

    if existing > 0 && !allow_empty {
        if empty_result {
            return Err(Refused::WouldEmpty { existing });
        }
        if guard > 0.0 && new_len > 0 && (existing as f64) / (new_len as f64) > guard {
            return Err(Refused::WouldShrink {
                existing,
                new: new_len,
                times: existing as f64 / new_len as f64,
            });
        }
    }

    // Атомарна заміна. tmp поруч із ціллю — щоб rename лишився в межах файлової системи:
    // через /tmp це був би крос-девайсний rename, який падає.
    let tmp = format!("{path}.tmp");
    {
        let mut f = std::fs::File::create(&tmp).map_err(Refused::Io)?;
        f.write_all(body.as_bytes()).map_err(Refused::Io)?;
        // sync_all перед rename: без нього після обриву живлення можна отримати
        // перейменований, але ще порожній файл — тобто рівно ту ж біду, лише пізніше.
        f.sync_all().map_err(Refused::Io)?;
    }
    std::fs::rename(&tmp, path).map_err(Refused::Io)?;
    Ok(())
}

/// Те саме для JSON: серіалізує з відступами й віддає в `save_str`.
pub fn save_json(path: &str, v: &serde_json::Value) -> Result<(), Refused> {
    let body = serde_json::to_string_pretty(v)
        .map_err(|e| Refused::Io(std::io::Error::other(e)))?;
    save_str(path, &body)
}

/// Зручний варіант для бінів: пише, а при відмові гучно скаржиться й ПАДАЄ.
///
/// Падіння тут навмисне. Тихе продовження після відмови означало б, що бін відзвітує
/// «done», а дані лишаться старими — тобто та сама неправда, лише з іншого боку.
pub fn save_json_or_die(tag: &str, path: &str, v: &serde_json::Value) {
    match save_json(path, v) {
        Ok(()) => {}
        Err(e) => {
            eprintln!("[{tag}] ⚠️ {path}: {e}");
            std::process::exit(3);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// `ALLOW_EMPTY_WRITE` — глобальний стан процесу, а тести біжать паралельно:
    /// тест з явним дозволом без цього замка вмикав його для сусідніх тестів,
    /// і ті випадково «проходили» захист, якого мали вимагати.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn env_guard() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn tmpdir() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("safewrite-test-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_first_write_always_succeeds() {
        let p = tmpdir().join("new.json");
        let p = p.to_str().unwrap();
        let _ = std::fs::remove_file(p);
        assert!(save_json(p, &json!({"a": 1})).is_ok());
        assert!(std::fs::metadata(p).unwrap().len() > 0);
    }

    #[test]
    fn an_empty_result_never_destroys_existing_data() {
        let _g = env_guard();
        let p = tmpdir().join("guard.json");
        let p = p.to_str().unwrap();
        save_json(p, &json!({"груп": (1..200).collect::<Vec<_>>()})).unwrap();
        let before = std::fs::read_to_string(p).unwrap();

        // саме те, що сталось 22.08: фільтр не знайшов нічого
        assert!(matches!(save_json(p, &json!({})), Err(Refused::WouldEmpty { .. })));
        assert_eq!(std::fs::read_to_string(p).unwrap(), before, "старі дані мають лишитись");
    }

    #[test]
    fn a_drastic_shrink_is_refused_too() {
        let _g = env_guard();
        let p = tmpdir().join("shrink.json");
        let p = p.to_str().unwrap();
        save_json(p, &json!({"x": (1..500).collect::<Vec<_>>()})).unwrap();
        assert!(matches!(save_json(p, &json!({"x": [1]})), Err(Refused::WouldShrink { .. })));
    }

    #[test]
    fn an_explicit_override_lets_a_real_emptying_through() {
        let _g = env_guard();
        let p = tmpdir().join("forced.json");
        let p = p.to_str().unwrap();
        save_json(p, &json!({"x": (1..200).collect::<Vec<_>>()})).unwrap();
        std::env::set_var("ALLOW_EMPTY_WRITE", "1");
        let r = save_json(p, &json!({}));
        std::env::remove_var("ALLOW_EMPTY_WRITE");
        assert!(r.is_ok(), "з явним дозволом порожній запис має пройти");
    }

    #[test]
    fn no_tmp_file_survives_a_successful_write() {
        let p = tmpdir().join("clean.json");
        let p = p.to_str().unwrap();
        save_json(p, &json!({"a": (1..100).collect::<Vec<_>>()})).unwrap();
        assert!(!std::path::Path::new(&format!("{p}.tmp")).exists(), "тимчасовий файл має зникнути");
    }
}
