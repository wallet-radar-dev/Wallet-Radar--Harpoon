//! wr-store — єдине сховище довідкових даних Wallet Radar.
//!
//! Правило проєкту: у `.rs` не лишається жодної адреси, порогу, групи чи параметра
//! стратегії. Усе це рядки в SQLite; код бере їх звідси.
//!
//! Продуктивність. Гарячі шляхи (`real_money(contract)` у циклі по 180 ГБ CSV)
//! не можуть ходити в БД на кожен рядок. Тому доступ двошаровий:
//! база — джерело правди, а `Snapshot` — незмінний зліпок у пам'яті, який
//! береться один раз на старті процесу.

pub mod addrs;
pub mod mirror;
pub mod secrets;

use rusqlite::Connection;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("сховища немає за шляхом {0} — спершу треба виконати імпорт (wr-import)")]
    Missing(PathBuf),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;

const MIGRATIONS: &[(i64, &str)] = &[
    (1, include_str!("../migrations/001_init.sql")),
    (2, include_str!("../migrations/002_tags.sql")),
];

/// Шлях до файла БД: `WR_DB` або `<DATASET_DIR|.>/wallet-radar.db`.
pub fn db_path() -> PathBuf {
    if let Ok(p) = std::env::var("WR_DB") {
        return PathBuf::from(p);
    }
    let dir = std::env::var("DATASET_DIR").unwrap_or_else(|_| ".".to_string());
    Path::new(&dir).join("wallet-radar.db")
}

/// Відкрити наявну базу. НЕ створює порожню: мовчазна порожня база гірша за відмову —
/// код побіг би з нульовими порогами й вирішив, що все гаразд.
pub fn open() -> Result<Connection> {
    let p = db_path();
    if !p.exists() {
        return Err(Error::Missing(p));
    }
    let c = Connection::open(&p)?;
    c.pragma_update(None, "foreign_keys", "ON")?;
    Ok(c)
}

/// Відкрити або створити й накотити міграції. Для імпортера й тестів.
pub fn open_or_create(path: &Path) -> Result<Connection> {
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir).ok();
        }
    }
    let mut c = Connection::open(path)?;
    // WAL ставимо ДО міграцій: усередині транзакції journal_mode змінити не можна.
    c.pragma_update(None, "journal_mode", "WAL")?;
    c.pragma_update(None, "foreign_keys", "ON")?;
    migrate(&mut c)?;
    Ok(c)
}

/// Ідемпотентна накатка міграцій під транзакцією.
pub fn migrate(c: &mut Connection) -> Result<()> {
    c.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
             version INTEGER PRIMARY KEY,
             applied_at TEXT NOT NULL DEFAULT (datetime('now')))",
    )?;
    let applied: i64 = c
        .query_row("SELECT COALESCE(MAX(version), 0) FROM schema_migrations", [], |r| r.get(0))?;
    for (v, sql) in MIGRATIONS {
        if *v <= applied {
            continue;
        }
        let tx = c.transaction()?;
        tx.execute_batch(sql)?;
        tx.execute("INSERT INTO schema_migrations(version) VALUES (?1)", [v])?;
        tx.commit()?;
    }
    Ok(())
}

/// Спільний знімок на весь процес. Береться лінько при першому зверненні.
///
/// Відмова тут — паніка з точною вказівкою, що робити. Це навмисно: альтернатива —
/// повернути нулі й дати системі працювати з вимкненими порогами, що виглядало б
/// як нормальна робота і псувало б кожен вердикт. Краще впасти на першому запиті.
pub fn snapshot() -> &'static Snapshot {
    use std::sync::OnceLock;
    static S: OnceLock<Snapshot> = OnceLock::new();
    S.get_or_init(|| match try_snapshot() {
        Ok(s) => s,
        Err(e) => panic!(
            "wr-store: довідники недоступні ({e}).\n\
             Сховище має бути за шляхом WR_DB (зараз: {}).\n\
             Полагодити: підключити диск або виконати `wr-import load`.",
            db_path().display()
        ),
    })
}

/// Те саме, але помилку віддає викликачеві — для бінарників, які хочуть
/// перевірити доступність на старті й самі вирішити, що робити.
pub fn try_snapshot() -> Result<Snapshot> {
    let c = open()?;
    Snapshot::load(&c)
}

/// Вид «справжніх грошей» для P&L — те, що раніше було `wr_dict::real_money`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Money {
    Usd,
    Eth,
    Matic,
}

impl Money {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "usd" => Some(Money::Usd),
            "eth" => Some(Money::Eth),
            "matic" => Some(Money::Matic),
            _ => None,
        }
    }
}

/// Незмінний зліпок довідників у пам'яті. Береться раз на процес.
#[derive(Debug, Default)]
pub struct Snapshot {
    constants: HashMap<String, (String, String)>, // key -> (value, kind)
    money: HashMap<(String, String), (Money, u32)>, // (chain, contract) -> (вид, decimals)
    wallet_kind: HashMap<(String, String), String>,
    wallet_tags: HashMap<(String, String), Vec<String>>,
    token_tags: HashMap<String, Vec<String>>, // tag -> контракти
    wallet_label: HashMap<(String, String), String>,
    strategy_params: HashMap<(String, String), String>,
    /// Скільки «грошових» токенів довелось пропустити через відсутні decimals.
    /// Видно через `counts()` — щоб це не було невидимою дірою в обліку.
    money_without_decimals: usize,
}

impl Snapshot {
    pub fn load(c: &Connection) -> Result<Self> {
        let mut s = Snapshot::default();

        let mut q = c.prepare("SELECT key, value, kind FROM constants")?;
        for row in q.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
        })? {
            let (k, v, kind) = row?;
            s.constants.insert(k, (v, kind));
        }

        let mut q = c.prepare(
            "SELECT chain, contract, money_kind, decimals FROM tokens WHERE money_kind IS NOT NULL",
        )?;
        for row in q.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<i64>>(3)?,
            ))
        })? {
            let (chain, contract, mk, dec) = row?;
            // decimals може бути NULL: частина переліків у коді була просто
            // множиною контрактів, без десяткових. Для P&L такий запис
            // непридатний — без decimals сира сума не переводиться в число.
            // Пропускаємо, але рахуємо: мовчазний пропуск тут означав би
            // «цей стейбл не гроші», і суми поїхали б без жодного сліду.
            match (Money::parse(&mk), dec) {
                (Some(m), Some(d)) => {
                    s.money.insert((chain, contract), (m, d as u32));
                }
                (Some(_), None) => s.money_without_decimals += 1,
                (None, _) => {}
            }
        }

        let mut q = c.prepare("SELECT chain, address, kind, label FROM wallets")?;
        for row in q.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
            ))
        })? {
            let (chain, addr, kind, label) = row?;
            if let Some(l) = label {
                s.wallet_label.insert((chain.clone(), addr.clone()), l);
            }
            s.wallet_kind.insert((chain, addr), kind);
        }

        let mut q = c.prepare("SELECT chain, address, tag FROM wallet_tags")?;
        for row in q.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
        })? {
            let (chain, addr, tag) = row?;
            s.wallet_tags.entry((chain, addr)).or_default().push(tag);
        }

        let mut q = c.prepare("SELECT tag, contract FROM token_tags")?;
        for row in q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
            let (tag, contract) = row?;
            s.token_tags.entry(tag).or_default().push(contract);
        }

        let mut q = c.prepare("SELECT strategy_id, key, value FROM strategy_params")?;
        for row in q.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
        })? {
            let (sid, k, v) = row?;
            s.strategy_params.insert((sid, k), v);
        }

        Ok(s)
    }

    /// Поріг/константа як f64. Відсутній ключ — це помилка конфігурації, не нуль:
    /// мовчазний нуль тут означав би «поріг вимкнено» і зіпсував би всі вердикти.
    pub fn num(&self, key: &str) -> Result<f64> {
        let (v, _) = self
            .constants
            .get(key)
            .ok_or_else(|| Error::Other(format!("константи '{key}' немає в базі")))?;
        v.parse::<f64>()
            .map_err(|_| Error::Other(format!("константа '{key}' не число: {v:?}")))
    }

    pub fn int(&self, key: &str) -> Result<i64> {
        Ok(self.num(key)? as i64)
    }

    pub fn text(&self, key: &str) -> Result<&str> {
        self.constants
            .get(key)
            .map(|(v, _)| v.as_str())
            .ok_or_else(|| Error::Other(format!("константи '{key}' немає в базі")))
    }

    /// Заміна `wr_dict::real_money()`.
    pub fn real_money(&self, chain: &str, contract: &str) -> Option<(Money, u32)> {
        self.money.get(&(chain.to_string(), contract.to_lowercase())).copied()
    }

    pub fn wallet_kind(&self, chain: &str, address: &str) -> Option<&str> {
        self.wallet_kind.get(&(chain.to_string(), address.to_lowercase())).map(|s| s.as_str())
    }

    pub fn wallet_label(&self, chain: &str, address: &str) -> Option<&str> {
        self.wallet_label.get(&(chain.to_string(), address.to_lowercase())).map(|s| s.as_str())
    }

    /// Чи має гаманець тег (членство в іменованому переліку, колишній `const …`).
    pub fn wallet_has_tag(&self, chain: &str, address: &str, tag: &str) -> bool {
        self.wallet_tags
            .get(&(chain.to_string(), address.to_lowercase()))
            .is_some_and(|v| v.iter().any(|t| t == tag))
    }

    /// Контракти з тегом. Порожній перелік — це майже напевно незавантажена база,
    /// тож викликач мусить вирішити, чи це припустимо; мовчки віддавати порожнечу
    /// у логіку, що очікує перелік, не можна.
    pub fn tokens_with_tag(&self, tag: &str) -> &[String] {
        self.token_tags.get(tag).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn param(&self, strategy: &str, key: &str) -> Result<&str> {
        self.strategy_params
            .get(&(strategy.to_string(), key.to_string()))
            .map(|s| s.as_str())
            .ok_or_else(|| Error::Other(format!("параметра '{strategy}.{key}' немає в базі")))
    }

    pub fn param_num(&self, strategy: &str, key: &str) -> Result<f64> {
        let v = self.param(strategy, key)?;
        v.parse::<f64>()
            .map_err(|_| Error::Other(format!("параметр '{strategy}.{key}' не число: {v:?}")))
    }

    pub fn counts(&self) -> (usize, usize, usize, usize) {
        (self.constants.len(), self.money.len(), self.wallet_kind.len(), self.strategy_params.len())
    }

    /// Токени з `money_kind`, але без `decimals` — для P&L непридатні.
    pub fn money_without_decimals(&self) -> usize {
        self.money_without_decimals
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem() -> Connection {
        let mut c = Connection::open_in_memory().unwrap();
        c.pragma_update(None, "foreign_keys", "ON").unwrap();
        migrate(&mut c).unwrap();
        c
    }

    #[test]
    fn migrations_are_idempotent() {
        let mut c = mem();
        let before: i64 =
            c.query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0)).unwrap();
        migrate(&mut c).unwrap(); // вдруге — має бути no-op, а не падіння
        let after: i64 =
            c.query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0)).unwrap();
        // Порівнюємо ДО і ПІСЛЯ, а не з конкретним числом: інакше тест довелось би
        // правити при кожній новій міграції, і одного разу його б просто оновили
        // під нове значення, не перевіривши, чи справді нічого не накотилось двічі.
        assert_eq!(before, after, "повторна накатка змінила журнал міграцій");
        assert_eq!(after as usize, MIGRATIONS.len(), "накотились не всі міграції");
    }

    #[test]
    fn addresses_must_be_lowercase() {
        let c = mem();
        c.execute("INSERT INTO chains VALUES ('eth','Ethereum','ETH',NULL,1)", []).unwrap();
        // Велика літера в адресі — найтихіша з можливих помилок: два записи про
        // один гаманець. База мусить відмовити, а не прийняти.
        let bad = c.execute(
            "INSERT INTO wallets(chain,address,kind,source) VALUES ('eth','0xAAbb','eoa','t')",
            [],
        );
        assert!(bad.is_err(), "БД прийняла адресу з великими літерами");
    }

    #[test]
    fn missing_constant_is_an_error_not_a_zero() {
        let c = mem();
        let s = Snapshot::load(&c).unwrap();
        assert!(s.num("HUB_PCT").is_err(), "відсутній поріг повернувся як значення");
    }

    #[test]
    fn real_money_roundtrip() {
        let c = mem();
        c.execute("INSERT INTO chains VALUES ('eth','Ethereum','ETH',NULL,1)", []).unwrap();
        c.execute(
            "INSERT INTO tokens(chain,contract,symbol,decimals,money_kind,source)
             VALUES ('eth','0xdac17f958d2ee523a2206206994597c13d831ec7','USDT',6,'usd','wr-dict')",
            [],
        )
        .unwrap();
        let s = Snapshot::load(&c).unwrap();
        // регістр на вході не має значення — нормалізується
        assert_eq!(
            s.real_money("eth", "0xDAC17F958D2ee523a2206206994597C13D831ec7"),
            Some((Money::Usd, 6))
        );
        assert_eq!(s.real_money("eth", "0xdead"), None);
    }
}
