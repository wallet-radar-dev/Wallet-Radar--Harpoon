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
pub mod model;
pub mod tags;
pub mod mirror;
pub mod secrets;
/// Журнал сигналів: єдине місце, де фіксується, що ми стверджували і чим це скінчилось.
/// Не довідник і не SQLite — навмисне, пояснення в самому модулі.
pub mod journal;

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
    (3, include_str!("../migrations/003_symbol_nullable.sql")),
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

pub use model::{Money, Role, Token, Wallet, WalletKind};

/// Незмінний зліпок довідників у пам'яті. Береться раз на процес.
#[derive(Debug, Default)]
pub struct Snapshot {
    constants: HashMap<String, (String, String)>,
    /// (chain, contract) -> токен. Один запис, а не три паралельні мапи:
    /// розсинхронізувати їх між собою вже неможливо.
    tokens: HashMap<(String, String), Token>,
    wallets: HashMap<(String, String), Wallet>,
    /// tag -> ключі у `wallets`. Зберігаємо ключі, а не копії: одна правда.
    wallet_tags: HashMap<String, Vec<(String, String)>>,
    token_tags: HashMap<String, Vec<(String, String)>>,
    /// role -> ключі у `wallets`.
    roles: HashMap<String, Vec<(String, String)>>,
    strategy_params: HashMap<(String, String), String>,
    /// Токени з `money_kind`, але без `decimals` — для лічби непридатні.
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
            "SELECT chain, contract, symbol, decimals, sector, money_kind FROM tokens",
        )?;
        for row in q.query_map([], |r| {
            Ok(Token {
                chain: r.get(0)?,
                contract: r.get(1)?,
                symbol: r.get(2)?,
                decimals: r.get::<_, Option<i64>>(3)?.map(|d| d as u32),
                sector: r.get(4)?,
                money: r.get::<_, Option<String>>(5)?.as_deref().and_then(Money::parse),
            })
        })? {
            let t = row?;
            // «гроші» без decimals порахувати не можна — рахуємо й НЕ ховаємо
            if t.money.is_some() && t.decimals.is_none() {
                s.money_without_decimals += 1;
            }
            s.tokens.insert((t.chain.clone(), t.contract.clone()), t);
        }

        let mut q = c.prepare("SELECT chain, address, kind, label FROM wallets")?;
        for row in q.query_map([], |r| {
            Ok(Wallet {
                chain: r.get(0)?,
                address: r.get(1)?,
                kind: WalletKind::parse(&r.get::<_, String>(2)?),
                label: r.get(3)?,
            })
        })? {
            let w = row?;
            s.wallets.insert((w.chain.clone(), w.address.clone()), w);
        }

        let mut q = c.prepare("SELECT tag, chain, address FROM wallet_tags")?;
        for row in q.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
        })? {
            let (tag, chain, addr) = row?;
            s.wallet_tags.entry(tag).or_default().push((chain, addr));
        }

        let mut q = c.prepare("SELECT tag, chain, contract FROM token_tags")?;
        for row in q.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
        })? {
            let (tag, chain, contract) = row?;
            s.token_tags.entry(tag).or_default().push((chain, contract));
        }

        let mut q = c.prepare("SELECT role, chain, address FROM wallet_roles")?;
        for row in q.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
        })? {
            let (role, chain, addr) = row?;
            s.roles.entry(role).or_default().push((chain, addr));
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

    /// Перевірка знімка на повноту: кожен тег, який ВЖИВАЄ код, має бути в базі.
    ///
    /// Без неї одрук в імені тегу — тиша: запит повертає порожньо, модуль
    /// вирішує, що перелік порожній, і працює далі. Тут це падає один раз,
    /// на старті, з переліком того, чого бракує.
    pub fn validate(&self) -> Result<()> {
        let mut missing: Vec<&str> = Vec::new();
        for t in tags::WALLET_TAGS {
            if !self.wallet_tags.contains_key(*t) {
                missing.push(t);
            }
        }
        for t in tags::TOKEN_TAGS {
            if !self.token_tags.contains_key(*t) {
                missing.push(t);
            }
        }
        if missing.is_empty() {
            return Ok(());
        }
        Err(Error::Other(format!(
            "у сховищі немає тегів, які вживає код: {}. \
             Схоже, база завантажена не до кінця — виконай `wr-import load`",
            missing.join(", ")
        )))
    }

    // ── вибірки ─────────────────────────────────────────────────────────────

    fn by_keys<'a, T>(
        map: &'a HashMap<(String, String), T>,
        keys: Option<&'a Vec<(String, String)>>,
    ) -> Vec<&'a T> {
        keys.map(|ks| ks.iter().filter_map(|k| map.get(k)).collect()).unwrap_or_default()
    }

    /// Гаманці з тегом. Порожній результат означає «тегу немає або він порожній» —
    /// щоб відрізнити одне від одного, є `validate()`.
    pub fn wallets_tagged(&self, tag: &str) -> Vec<&Wallet> {
        Self::by_keys(&self.wallets, self.wallet_tags.get(tag))
    }

    /// Токени з тегом.
    pub fn tokens_tagged(&self, tag: &str) -> Vec<&Token> {
        Self::by_keys(&self.tokens, self.token_tags.get(tag))
    }

    /// Гаманці з роллю у розслідуванні.
    pub fn wallets_with_role(&self, role: Role) -> Vec<&Wallet> {
        Self::by_keys(&self.wallets, self.roles.get(role.as_str()))
    }

    /// Чи має адреса тег. Дешевше за `wallets_tagged`, коли потрібна лише перевірка.
    pub fn has_tag(&self, chain: &str, address: &str, tag: &str) -> bool {
        let key = (chain.to_string(), address.to_lowercase());
        self.wallet_tags.get(tag).is_some_and(|v| v.contains(&key))
    }

    /// Гаманець за адресою.
    pub fn wallet(&self, chain: &str, address: &str) -> Option<&Wallet> {
        self.wallets.get(&(chain.to_string(), address.to_lowercase()))
    }

    /// Токен за контрактом.
    pub fn token(&self, chain: &str, contract: &str) -> Option<&Token> {
        self.tokens.get(&(chain.to_string(), contract.to_lowercase()))
    }

    /// Перший токен із таким символом. `None`, якщо символ невідомий у довіднику.
    pub fn token_by_symbol(&self, chain: &str, symbol: &str) -> Option<&Token> {
        self.tokens
            .values()
            .find(|t| t.chain == chain && t.symbol.as_deref() == Some(symbol))
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

    /// «Справжні гроші»: вид і десяткові. `None`, якщо це не гроші АБО
    /// якщо десяткових не знаємо — рахувати суму в обох випадках не можна.
    pub fn real_money(&self, chain: &str, contract: &str) -> Option<(Money, u32)> {
        let t = self.token(chain, contract)?;
        Some((t.money?, t.decimals?))
    }

    pub fn wallet_kind(&self, chain: &str, address: &str) -> Option<&str> {
        self.wallet(chain, address).map(|w| w.kind.as_str())
    }

    pub fn wallet_label(&self, chain: &str, address: &str) -> Option<&str> {
        self.wallet(chain, address).and_then(|w| w.label.as_deref())
    }

    /// Чи має гаманець тег (членство в іменованому переліку, колишній `const …`).
    /// Сумісність зі старим ім'ям; віддає перевагу `has_tag`.
    pub fn wallet_has_tag(&self, chain: &str, address: &str, tag: &str) -> bool {
        self.has_tag(chain, address, tag)
    }

    /// Контракти з тегом. Порожній перелік — це майже напевно незавантажена база,
    /// тож викликач мусить вирішити, чи це припустимо; мовчки віддавати порожнечу
    /// у логіку, що очікує перелік, не можна.
    /// Контракти з тегом. Для повних записів — `tokens_tagged`.
    pub fn token_contracts_with_tag(&self, tag: &str) -> Vec<&str> {
        self.tokens_tagged(tag).into_iter().map(|t| t.contract.as_str()).collect()
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
        (self.constants.len(), self.tokens.len(), self.wallets.len(), self.strategy_params.len())
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
