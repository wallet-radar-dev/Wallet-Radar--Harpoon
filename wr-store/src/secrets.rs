//! Секрети: значення живуть у Windows Credential Manager (DPAPI, під обліковим
//! записом користувача), а не у файлах .env і не в базі.
//!
//! Чому не в базі: файл БД копіюється в бекапи й на зовнішній диск. Ключі поїхали б
//! разом із ним. У базі лежить лише РЕЄСТР (таблиця `secrets`) — імена й те, де шукати.
//!
//! Env залишається як явний фолбек: на headless-Linux (ВМ під systemd) сховища
//! Secret Service немає, і там єдиний робочий шлях — змінна оточення.

use crate::{Error, Result};
use rusqlite::Connection;

/// Звідки взялося значення — щоб у логах було видно, яким шляхом воно прийшло.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Keyring,
    Env,
}

#[derive(Debug, Clone)]
pub struct SecretSpec {
    pub name: String,
    pub service: String,
    pub account: String,
    pub required: bool,
    pub env_fallback: Option<String>,
    pub description: String,
}

pub fn specs(c: &Connection) -> Result<Vec<SecretSpec>> {
    let mut q = c.prepare(
        "SELECT name, service, account, required, env_fallback, description
         FROM secrets ORDER BY name",
    )?;
    let rows = q.query_map([], |r| {
        Ok(SecretSpec {
            name: r.get(0)?,
            service: r.get(1)?,
            account: r.get(2)?,
            required: r.get::<_, i64>(3)? != 0,
            env_fallback: r.get(4)?,
            description: r.get(5)?,
        })
    })?;
    rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Error::from)
}

fn spec(c: &Connection, name: &str) -> Result<SecretSpec> {
    specs(c)?
        .into_iter()
        .find(|s| s.name == name)
        .ok_or_else(|| Error::Other(format!("секрета '{name}' немає в реєстрі (таблиця secrets)")))
}

/// Прочитати секрет: спершу Credential Manager, потім env-фолбек.
pub fn get(c: &Connection, name: &str) -> Result<(String, Origin)> {
    let s = spec(c, name)?;
    match keyring::Entry::new(&s.service, &s.account).and_then(|e| e.get_password()) {
        Ok(v) if !v.is_empty() => return Ok((v, Origin::Keyring)),
        Ok(_) => {}
        Err(keyring::Error::NoEntry) => {}
        Err(e) => return Err(Error::Other(format!("Credential Manager для '{name}': {e}"))),
    }
    if let Some(var) = &s.env_fallback {
        if let Ok(v) = std::env::var(var) {
            if !v.is_empty() {
                return Ok((v, Origin::Env));
            }
        }
    }
    Err(Error::Other(format!(
        "секрет '{name}' не знайдено ні в Credential Manager ({}/{}), ні в env{}",
        s.service,
        s.account,
        s.env_fallback.map(|v| format!(" ({v})")).unwrap_or_default()
    )))
}

/// Записати секрет у Credential Manager.
pub fn set(c: &Connection, name: &str, value: &str) -> Result<()> {
    if value.is_empty() {
        return Err(Error::Other(format!("порожнє значення для секрета '{name}'")));
    }
    let s = spec(c, name)?;
    keyring::Entry::new(&s.service, &s.account)
        .and_then(|e| e.set_password(value))
        .map_err(|e| Error::Other(format!("запис '{name}' у Credential Manager: {e}")))
}

pub fn delete(c: &Connection, name: &str) -> Result<()> {
    let s = spec(c, name)?;
    match keyring::Entry::new(&s.service, &s.account).and_then(|e| e.delete_credential()) {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(Error::Other(format!("видалення '{name}': {e}"))),
    }
}

#[derive(Debug)]
pub struct AuditRow {
    pub name: String,
    pub required: bool,
    pub found: Option<Origin>,
}

/// Перевірка на старті: які з потрібних секретів є і звідки.
///
/// Це прямо лікує випадок, коли переїзд env-файлів на тиждень поклав `cexsignal`:
/// ключа не було, а процес мовчки працював далі.
pub fn audit(c: &Connection) -> Result<Vec<AuditRow>> {
    let mut out = Vec::new();
    for s in specs(c)? {
        let found = get(c, &s.name).ok().map(|(_, o)| o);
        out.push(AuditRow { name: s.name, required: s.required, found });
    }
    Ok(out)
}

/// Відсутні ОБОВ'ЯЗКОВІ секрети. Порожній вектор = можна стартувати.
pub fn missing_required(c: &Connection) -> Result<Vec<String>> {
    Ok(audit(c)?
        .into_iter()
        .filter(|r| r.required && r.found.is_none())
        .map(|r| r.name)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrate;

    /// Кожен тест дістає ВЛАСНЕ ім'я env-змінної.
    ///
    /// Раніше всі ділили одну `WR_TEST_ENV_KEY` — і тести стали флакі: cargo
    /// ганяє їх паралельно в одному процесі, тож `missing_required` бачив змінну,
    /// яку саме виставив сусідній тест. Спіймано на випадковому падінні 1 із 12.
    /// Змінні оточення — процесо-глобальний стан, спільний для всіх потоків.
    fn mem(tag: &str) -> (Connection, String) {
        let mut c = Connection::open_in_memory().unwrap();
        migrate(&mut c).unwrap();
        let var = format!("WR_TEST_ENV_{}", tag.to_uppercase());
        c.execute(
            "INSERT INTO secrets(name,service,account,required,env_fallback,description)
             VALUES ('TEST_KEY','wallet-radar-test',?1,1,?2,'тест')",
            (tag, &var),
        )
        .unwrap();
        (c, var)
    }

    #[test]
    fn unknown_secret_names_itself_in_the_error() {
        let (c, _v) = mem("unknown");
        let e = get(&c, "NOPE").unwrap_err().to_string();
        assert!(e.contains("NOPE"), "помилка не називає секрет: {e}");
    }

    #[test]
    fn env_fallback_is_used_and_reported_as_env() {
        let (c, var) = mem("fallback");
        // SAFETY: ім'я змінної унікальне для цього тесту, тож інші потоки
        // її не читають і не пишуть.
        unsafe { std::env::set_var(&var, "from-env") };
        let (v, origin) = get(&c, "TEST_KEY").unwrap();
        assert_eq!(v, "from-env");
        assert_eq!(origin, Origin::Env, "джерело секрета вказано невірно");
        unsafe { std::env::remove_var(&var) };
    }

    #[test]
    fn missing_required_lists_the_gap() {
        let (c, _v) = mem("missing");
        let m = missing_required(&c).unwrap();
        assert_eq!(m, vec!["TEST_KEY".to_string()]);
    }

    #[test]
    fn empty_value_is_refused() {
        let (c, _v) = mem("empty");
        assert!(set(&c, "TEST_KEY", "").is_err(), "порожній секрет записався");
    }
}
