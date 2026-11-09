//! Реалізація сховища на SQLite (rusqlite).
//!
//! `rusqlite` синхронний, тому кожна операція виконується у
//! `tokio::task::spawn_blocking`: блокування відбувається у пулі блокуючих
//! задач, а не на потоках async-екзекутора.
//!
//! З'єднання обгорнуте в `Arc<Mutex<Connection>>`; клоч `Arc` копіюється
//! перед переміщенням у блокуючу задачу, а блокіровка береться вже всередині
//! задачі ( MutexGuard не є `Send`, тож не можна переносити його через await).

use std::sync::{Arc, Mutex};

use rusqlite::{params, Connection, OpenFlags, OptionalExtension, Row, TransactionBehavior};
use sha2::{Digest, Sha256};

/// Псевдонім чату: HMAC від chat_id із секретом.
fn chat_ref_with(secret: &[u8], chat_id: i64) -> String {
    let mut mac = HmacSha256::new(secret);
    mac.update(&chat_id.to_le_bytes());
    format!("chat_{}", &mac.finish_hex()[..16])
}

/// HMAC-SHA256 — щоб `chat_ref` у аудиті не можна було відновити перебором.
struct HmacSha256 {
    inner: Sha256,
    outer_key: [u8; 64],
}

impl HmacSha256 {
    fn new(key: &[u8]) -> HmacSha256 {
        let mut block = [0u8; 64];
        if key.len() > 64 {
            let digest = Sha256::digest(key);
            block[..32].copy_from_slice(&digest);
        } else {
            block[..key.len()].copy_from_slice(key);
        }
        let mut ipad = [0x36u8; 64];
        let mut opad = [0x5cu8; 64];
        for i in 0..64 {
            ipad[i] ^= block[i];
            opad[i] ^= block[i];
        }
        let mut inner = Sha256::new();
        inner.update(ipad);
        HmacSha256 {
            inner,
            outer_key: opad,
        }
    }

    fn update(&mut self, data: &[u8]) {
        self.inner.update(data);
    }

    fn finish_hex(self) -> String {
        let inner = self.inner.finalize();
        let mut outer = Sha256::new();
        outer.update(self.outer_key);
        outer.update(inner);
        outer
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }
}

use crate::{
    now_ms, AlertRecord, AuditEntry, Delivery, DeliveryStatus, DeliveryUpdate, Language,
    NewDelivery, NewUser, Priority, Role, Settings, Store, StoreError, Subscription, Timeframe,
    Topic, User, UserState,
};

/// Схема БД. Використовується `IF NOT EXISTS`, тому повторний виклик безпечний.
const SCHEMA: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;
CREATE TABLE IF NOT EXISTS users (
  chat_id BIGINT PRIMARY KEY, user_id BIGINT, username TEXT,
  role TEXT NOT NULL DEFAULT 'viewer', language TEXT NOT NULL DEFAULT 'uk',
  timezone TEXT NOT NULL DEFAULT 'UTC', timeframe TEXT NOT NULL DEFAULT '24h',
  muted INTEGER NOT NULL DEFAULT 0, blocked INTEGER NOT NULL DEFAULT 0,
  state TEXT NOT NULL DEFAULT 'active',
  created_at BIGINT NOT NULL, updated_at BIGINT NOT NULL, last_seen_at BIGINT);
CREATE UNIQUE INDEX IF NOT EXISTS users_user_id_idx ON users(user_id) WHERE user_id IS NOT NULL;
CREATE TABLE IF NOT EXISTS subscriptions (
  chat_id BIGINT NOT NULL REFERENCES users(chat_id) ON DELETE CASCADE,
  topic TEXT NOT NULL, min_severity TEXT NOT NULL DEFAULT 'medium',
  PRIMARY KEY (chat_id, topic));
CREATE TABLE IF NOT EXISTS user_assets (
  chat_id BIGINT NOT NULL REFERENCES users(chat_id) ON DELETE CASCADE,
  symbol TEXT NOT NULL, PRIMARY KEY (chat_id, symbol));
CREATE TABLE IF NOT EXISTS alert_log (
  alert_key TEXT PRIMARY KEY, kind TEXT NOT NULL, asset TEXT, window_start BIGINT NOT NULL,
  severity TEXT NOT NULL, degraded INTEGER NOT NULL DEFAULT 0, payload TEXT, created_at BIGINT NOT NULL);
CREATE INDEX IF NOT EXISTS alert_log_time_idx ON alert_log (created_at);
CREATE TABLE IF NOT EXISTS deliveries (
  chat_id BIGINT NOT NULL, alert_key TEXT NOT NULL, priority SMALLINT NOT NULL,
  status TEXT NOT NULL, attempts INTEGER NOT NULL DEFAULT 0, next_attempt_at BIGINT NOT NULL,
  created_at BIGINT NOT NULL, sent_at BIGINT, last_error TEXT, text TEXT NOT NULL DEFAULT '',
  PRIMARY KEY (chat_id, alert_key));
CREATE INDEX IF NOT EXISTS deliveries_due_idx ON deliveries (status, next_attempt_at, priority);
CREATE TABLE IF NOT EXISTS seen_events (event_id TEXT PRIMARY KEY, ts BIGINT NOT NULL);
CREATE INDEX IF NOT EXISTS seen_events_ts_idx ON seen_events (ts);
CREATE TABLE IF NOT EXISTS seen_updates (update_id BIGINT PRIMARY KEY, seen_at BIGINT NOT NULL);
CREATE TABLE IF NOT EXISTS snapshots (as_of BIGINT PRIMARY KEY, kind TEXT NOT NULL DEFAULT 'full', payload TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS audit_log (
  id INTEGER PRIMARY KEY AUTOINCREMENT, chat_ref TEXT NOT NULL, actor TEXT NOT NULL,
  action TEXT NOT NULL, detail TEXT, at BIGINT NOT NULL);
CREATE INDEX IF NOT EXISTS audit_log_ref_idx ON audit_log (chat_ref, at);
CREATE TABLE IF NOT EXISTS kv (k TEXT PRIMARY KEY, v TEXT NOT NULL, updated_at BIGINT NOT NULL);
"#;

/// Сховище стану на базі SQLite.
#[derive(Clone)]
pub struct SqliteStore {
    conn: Arc<Mutex<Connection>>,
    /// Секрет для підписування `chat_ref`. Порожній — підписування без ключа
    /// (лише хеш): усе одно краще, ніж сам chat_id у логах.
    secret: Arc<Vec<u8>>,
}

impl SqliteStore {
    /// Відкрити з секретом підписування `chat_ref`.
    ///
    /// `chat_ref` — це те, що потрапляє в аудит і логи замість chat_id. Без
    /// секрету це просто хеш chat_id (відновити можна перебором), із секретом —
    /// HMAC, який без ключа не відновити.
    pub fn open_with_secret<P: AsRef<std::path::Path>>(
        path: P,
        secret: &[u8],
        busy_timeout_ms: u64,
    ) -> Result<Self, StoreError> {
        let store = Self::open(path)?;
        store
            .conn
            .lock()
            .map_err(|_| StoreError::LockPoisoned)?
            .busy_timeout(std::time::Duration::from_millis(busy_timeout_ms))?;
        Ok(SqliteStore {
            conn: store.conn,
            secret: Arc::new(secret.to_vec()),
        })
    }

    /// Псевдонім чату для аудиту й логів.
    pub fn chat_ref(&self, chat_id: i64) -> String {
        chat_ref_with(&self.secret, chat_id)
    }

    /// Відкриває (або створює) файл бази даних.
    pub fn open<P: AsRef<std::path::Path>>(path: P) -> Result<Self, StoreError> {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_URI
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        let store = Self::init(conn)?;
        Ok(SqliteStore {
            conn: store.conn,
            secret: Arc::new(Vec::new()),
        })
    }

    /// Відкриває базу в оперативній пам'яті (для тестів).
    pub fn open_in_memory() -> Result<Self, StoreError> {
        let conn = Connection::open_in_memory()?;
        let store = Self::init(conn)?;
        Ok(SqliteStore {
            conn: store.conn,
            secret: Arc::new(Vec::new()),
        })
    }

    fn init(conn: Connection) -> Result<Self, StoreError> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.execute_batch(SCHEMA)?;
        // Міграція для баз, створених до появи колонки chat_ref.
        let has_chat_ref: bool = conn
            .prepare("PRAGMA table_info(users)")?
            .query_map([], |r| r.get::<_, String>(1))?
            .collect::<Result<Vec<String>, _>>()?
            .iter()
            .any(|c| c == "chat_ref");
        if !has_chat_ref {
            conn.execute("ALTER TABLE users ADD COLUMN chat_ref TEXT", [])?;
            conn.execute(
                "UPDATE users SET chat_ref = 'chat:' || chat_id WHERE chat_ref IS NULL OR chat_ref = ''",
                [],
            )?;
        }
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            secret: Arc::new(Vec::new()),
        })
    }

    /// Виконує блокуючу операцію над БД поза async-екзекутором.
    async fn call<T, F>(&self, f: F) -> Result<T, StoreError>
    where
        F: FnOnce(&mut Connection) -> Result<T, StoreError> + Send + 'static,
        T: Send + 'static,
    {
        // Копія Arc копіюється до переміщення в задачу: сам Arc<Mutex<..>>
        // є Send, а отже блокіровку можна взяти всередині блокуючої задачі.
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let mut guard = conn.lock().map_err(|_| StoreError::LockPoisoned)?;
            f(&mut guard)
        })
        .await?
    }
}

/// Читає колонки користувача у вказаному порядку.
const USER_COLS: &str = "chat_id, user_id, username, role, language, timezone, timeframe, muted, \
blocked, state, created_at, updated_at, last_seen_at";

fn row_to_user(row: &Row<'_>) -> rusqlite::Result<User> {
    let role: String = row.get(3)?;
    let language: String = row.get(4)?;
    let timeframe: String = row.get(6)?;
    let state: String = row.get(9)?;
    Ok(User {
        chat_id: row.get(0)?,
        user_id: row.get(1)?,
        username: row.get(2)?,
        role: Role::from_str_code(&role),
        language: Language::from_str_code(&language),
        timezone: row.get(5)?,
        timeframe: Timeframe::from_str_code(&timeframe),
        muted: row.get::<_, i64>(7)? != 0,
        blocked: row.get::<_, i64>(8)? != 0,
        state: UserState::from_str_code(&state),
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
        last_seen_at: row.get(12)?,
    })
}

fn row_to_delivery(row: &Row<'_>) -> rusqlite::Result<Delivery> {
    let priority: i64 = row.get(2)?;
    let status: String = row.get(3)?;
    Ok(Delivery {
        chat_id: row.get(0)?,
        alert_key: row.get(1)?,
        priority: Priority::from_i64(priority),
        text: row.get(7)?,
        status: DeliveryStatus::from_str_code(&status),
        attempts: row.get::<_, i64>(4)? as u32,
        next_attempt_at: row.get(5)?,
        created_at: row.get(6)?,
    })
}

impl Store for SqliteStore {
    fn chat_ref(&self, chat_id: i64) -> String {
        SqliteStore::chat_ref(self, chat_id)
    }

    async fn upsert_user(&self, u: NewUser) -> Result<User, StoreError> {
        let chat_ref = SqliteStore::chat_ref(self, u.chat_id);
        self.call(move |conn| {
            let now = now_ms();
            conn.execute(
                "INSERT INTO users (chat_id, user_id, username, role, language, timezone, chat_ref, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)
                 ON CONFLICT(chat_id) DO UPDATE SET
                   user_id = COALESCE(excluded.user_id, users.user_id),
                   username = excluded.username,
                   role = excluded.role,
                   language = excluded.language,
                   timezone = excluded.timezone,
                   updated_at = excluded.updated_at",
                params![
                    u.chat_id,
                    u.user_id,
                    u.username,
                    u.role.as_str(),
                    u.language.as_str(),
                    u.timezone,
                    chat_ref,
                    now
                ],
            )?;
            let sql = format!("SELECT {USER_COLS} FROM users WHERE chat_id = ?1");
            let user = conn
                .query_row(&sql, params![u.chat_id], row_to_user)
                .optional()?
                .ok_or(StoreError::NotFound(u.chat_id))?;
            Ok(user)
        })
        .await
    }

    async fn get_user(&self, chat_id: i64) -> Result<Option<User>, StoreError> {
        self.call(move |conn| {
            let sql = format!("SELECT {USER_COLS} FROM users WHERE chat_id = ?1");
            Ok(conn
                .query_row(&sql, params![chat_id], row_to_user)
                .optional()?)
        })
        .await
    }

    async fn find_user_by_user_id(&self, user_id: i64) -> Result<Option<User>, StoreError> {
        self.call(move |conn| {
            let sql = format!("SELECT {USER_COLS} FROM users WHERE user_id = ?1");
            Ok(conn
                .query_row(&sql, params![user_id], row_to_user)
                .optional()?)
        })
        .await
    }

    async fn set_settings(&self, chat_id: i64, s: &Settings) -> Result<(), StoreError> {
        let s = s.clone();
        self.call(move |conn| {
            let changed = conn.execute(
                "UPDATE users SET language = ?2, timezone = ?3, timeframe = ?4, muted = ?5, updated_at = ?6
                 WHERE chat_id = ?1",
                params![
                    chat_id,
                    s.language.as_str(),
                    s.timezone,
                    s.timeframe.as_str(),
                    i64::from(s.muted),
                    now_ms()
                ],
            )?;
            if changed == 0 {
                return Err(StoreError::NotFound(chat_id));
            }
            Ok(())
        })
        .await
    }

    async fn set_last_seen(&self, chat_id: i64, at: i64) -> Result<(), StoreError> {
        self.call(move |conn| {
            let changed = conn.execute(
                "UPDATE users SET last_seen_at = ?2, updated_at = ?2 WHERE chat_id = ?1",
                params![chat_id, at],
            )?;
            if changed == 0 {
                return Err(StoreError::NotFound(chat_id));
            }
            Ok(())
        })
        .await
    }

    async fn set_blocked(&self, chat_id: i64, blocked: bool) -> Result<(), StoreError> {
        self.call(move |conn| {
            let changed = conn.execute(
                "UPDATE users SET blocked = ?2, updated_at = ?3 WHERE chat_id = ?1",
                params![chat_id, i64::from(blocked), now_ms()],
            )?;
            if changed == 0 {
                return Err(StoreError::NotFound(chat_id));
            }
            Ok(())
        })
        .await
    }

    async fn subscriptions(&self, chat_id: i64) -> Result<Vec<Subscription>, StoreError> {
        self.call(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT topic, min_severity FROM subscriptions WHERE chat_id = ?1 ORDER BY topic",
            )?;
            let rows = stmt.query_map(params![chat_id], |row| {
                let topic: String = row.get(0)?;
                Ok(Subscription {
                    chat_id,
                    topic: Topic::from_str_code(&topic),
                    min_severity: row.get(1)?,
                })
            })?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r?);
            }
            Ok(out)
        })
        .await
    }

    async fn set_subscription(&self, s: &Subscription) -> Result<(), StoreError> {
        let s = s.clone();
        self.call(move |conn| {
            conn.execute(
                "INSERT INTO subscriptions (chat_id, topic, min_severity) VALUES (?1, ?2, ?3)
                 ON CONFLICT(chat_id, topic) DO UPDATE SET min_severity = excluded.min_severity",
                params![s.chat_id, s.topic.as_str(), s.min_severity],
            )?;
            Ok(())
        })
        .await
    }

    async fn remove_subscription(&self, chat_id: i64, topic: Topic) -> Result<(), StoreError> {
        self.call(move |conn| {
            conn.execute(
                "DELETE FROM subscriptions WHERE chat_id = ?1 AND topic = ?2",
                params![chat_id, topic.as_str()],
            )?;
            Ok(())
        })
        .await
    }

    async fn user_assets(&self, chat_id: i64) -> Result<Vec<String>, StoreError> {
        self.call(move |conn| {
            let mut stmt =
                conn.prepare("SELECT symbol FROM user_assets WHERE chat_id = ?1 ORDER BY symbol")?;
            let rows = stmt.query_map(params![chat_id], |row| row.get::<_, String>(0))?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r?);
            }
            Ok(out)
        })
        .await
    }

    async fn set_user_asset(&self, chat_id: i64, symbol: &str, on: bool) -> Result<(), StoreError> {
        let symbol = symbol.trim().to_ascii_uppercase();
        self.call(move |conn| {
            if on {
                conn.execute(
                    "INSERT OR IGNORE INTO user_assets (chat_id, symbol) VALUES (?1, ?2)",
                    params![chat_id, symbol],
                )?;
            } else {
                conn.execute(
                    "DELETE FROM user_assets WHERE chat_id = ?1 AND symbol = ?2",
                    params![chat_id, symbol],
                )?;
            }
            Ok(())
        })
        .await
    }

    async fn register_alert(&self, a: &AlertRecord) -> Result<bool, StoreError> {
        let a = a.clone();
        self.call(move |conn| {
            let changed = conn.execute(
                "INSERT INTO alert_log (alert_key, kind, asset, window_start, severity, degraded, payload, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(alert_key) DO NOTHING",
                params![
                    a.alert_key,
                    a.kind,
                    a.asset,
                    a.window_start,
                    a.severity,
                    i64::from(a.degraded),
                    a.payload,
                    a.created_at
                ],
            )?;
            Ok(changed > 0)
        })
        .await
    }

    async fn alert_exists(&self, alert_key: &str) -> Result<bool, StoreError> {
        let key = alert_key.to_string();
        self.call(move |conn| {
            let found: Option<i64> = conn
                .query_row(
                    "SELECT 1 FROM alert_log WHERE alert_key = ?1",
                    params![key],
                    |r| r.get(0),
                )
                .optional()?;
            Ok(found.is_some())
        })
        .await
    }

    async fn enqueue_delivery(&self, d: NewDelivery) -> Result<(), StoreError> {
        self.call(move |conn| {
            conn.execute(
                "INSERT OR IGNORE INTO deliveries
                   (chat_id, alert_key, priority, status, attempts, next_attempt_at, created_at, text)
                 VALUES (?1, ?2, ?3, 'pending', 0, ?4, ?5, ?6)",
                params![
                    d.chat_id,
                    d.alert_key,
                    d.priority.as_i64(),
                    d.created_at,
                    d.created_at,
                    d.text
                ],
            )?;
            Ok(())
        })
        .await
    }

    async fn claim_deliveries(&self, now: i64, limit: u32) -> Result<Vec<Delivery>, StoreError> {
        self.call(move |conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let rowids: Vec<i64> = {
                let mut stmt = tx.prepare(
                    "SELECT rowid FROM deliveries
                     WHERE status = 'pending' AND next_attempt_at <= ?1
                     ORDER BY priority ASC, next_attempt_at ASC LIMIT ?2",
                )?;
                let rows = stmt.query_map(params![now, i64::from(limit)], |r| r.get::<_, i64>(0))?;
                let mut ids = Vec::new();
                for r in rows {
                    ids.push(r?);
                }
                ids
            };
            if rowids.is_empty() {
                tx.commit()?;
                return Ok(Vec::new());
            }
            // Атомарний перевід pending -> sending для відібраних рядків.
            {
                let mut stmt = tx.prepare(
                    "UPDATE deliveries SET status = 'sending'
                     WHERE rowid IN (SELECT rowid FROM deliveries
                     WHERE status = 'pending' AND next_attempt_at <= ?1
                     ORDER BY priority ASC, next_attempt_at ASC LIMIT ?2)",
                )?;
                stmt.execute(params![now, i64::from(limit)])?;
            }
            // Читаємо саме ті рядки, які щойно переведені у sending.
            let placeholders = std::iter::repeat_n("?", rowids.len())
                .collect::<Vec<_>>()
                .join(", ");
            let mut deliveries = Vec::with_capacity(rowids.len());
            {
                let sql = format!(
                    "SELECT chat_id, alert_key, priority, status, attempts, next_attempt_at, created_at, text
                     FROM deliveries WHERE rowid IN ({placeholders})
                     ORDER BY priority ASC, next_attempt_at ASC"
                );
                let mut stmt = tx.prepare(&sql)?;
                let rows = stmt.query_map(rusqlite::params_from_iter(rowids.iter()), row_to_delivery)?;
                for r in rows {
                    deliveries.push(r?);
                }
            }
            tx.commit()?;
            Ok(deliveries)
        })
        .await
    }

    async fn finish_delivery(&self, d: DeliveryUpdate) -> Result<(), StoreError> {
        self.call(move |conn| {
            // sent_at заповнюється лише для фінального успіху; для повтору
            // лишається NULL, щоб було видно незавершені доставки.
            conn.execute(
                "UPDATE deliveries SET status = ?3, attempts = ?4, next_attempt_at = ?5, last_error = ?6,
                   sent_at = CASE WHEN ?3 = 'sent' THEN ?7 ELSE sent_at END
                 WHERE chat_id = ?1 AND alert_key = ?2",
                params![
                    d.chat_id,
                    d.alert_key,
                    d.status.as_str(),
                    i64::from(d.attempts),
                    d.next_attempt_at,
                    d.error,
                    now_ms()
                ],
            )?;
            Ok(())
        })
        .await
    }

    async fn recipients_for(&self, topic: Topic) -> Result<Vec<i64>, StoreError> {
        self.call(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT DISTINCT u.chat_id FROM users u
                 JOIN subscriptions s ON s.chat_id = u.chat_id
                 WHERE s.topic = ?1 AND u.muted = 0 AND u.blocked = 0 AND u.state = 'active'
                 ORDER BY u.chat_id",
            )?;
            let rows = stmt.query_map(params![topic.as_str()], |r| r.get::<_, i64>(0))?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r?);
            }
            Ok(out)
        })
        .await
    }

    async fn count_recipients(&self) -> Result<i64, StoreError> {
        self.call(|conn| {
            let n: i64 = conn.query_row(
                "SELECT COUNT(*) FROM users WHERE muted = 0 AND blocked = 0 AND state = 'active'",
                [],
                |r| r.get(0),
            )?;
            Ok(n)
        })
        .await
    }

    async fn all_active_chats(&self) -> Result<Vec<i64>, StoreError> {
        self.call(|conn| {
            let mut stmt = conn.prepare(
                "SELECT chat_id FROM users
                 WHERE muted = 0 AND blocked = 0 AND state = 'active'
                 ORDER BY chat_id",
            )?;
            let rows = stmt.query_map([], |r| r.get::<_, i64>(0))?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r?);
            }
            Ok(out)
        })
        .await
    }

    async fn list_users(&self) -> Result<Vec<User>, StoreError> {
        self.call(|conn| {
            let sql = format!("SELECT {USER_COLS} FROM users ORDER BY chat_id");
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map([], row_to_user)?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r?);
            }
            Ok(out)
        })
        .await
    }

    async fn alerts_since(&self, since_ms: i64) -> Result<Vec<AlertRecord>, StoreError> {
        self.call(move |conn| {
            // Параметризований запит: значення не зливаються в текст SQL.
            let mut stmt = conn.prepare(
                "SELECT alert_key, kind, asset, window_start, severity, degraded, payload, created_at
                 FROM alert_log WHERE created_at >= ?1 ORDER BY created_at DESC, alert_key",
            )?;
            let rows = stmt.query_map(params![since_ms], |r| {
                Ok(AlertRecord {
                    alert_key: r.get(0)?,
                    kind: r.get(1)?,
                    asset: r.get(2)?,
                    window_start: r.get(3)?,
                    severity: r.get(4)?,
                    degraded: r.get::<_, i64>(5)? != 0,
                    payload: r.get::<_, Option<String>>(6)?.unwrap_or_default(),
                    created_at: r.get(7)?,
                })
            })?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r?);
            }
            Ok(out)
        })
        .await
    }

    async fn seen_event(&self, event_id: &str, ts: i64) -> Result<bool, StoreError> {
        let event_id = event_id.to_string();
        self.call(move |conn| {
            let changed = conn.execute(
                "INSERT OR IGNORE INTO seen_events (event_id, ts) VALUES (?1, ?2)",
                params![event_id, ts],
            )?;
            Ok(changed > 0)
        })
        .await
    }

    async fn seen_update(&self, update_id: i64) -> Result<bool, StoreError> {
        self.call(move |conn| {
            let changed = conn.execute(
                "INSERT OR IGNORE INTO seen_updates (update_id, seen_at) VALUES (?1, ?2)",
                params![update_id, now_ms()],
            )?;
            Ok(changed > 0)
        })
        .await
    }

    async fn cursor(&self, name: &str) -> Result<Option<String>, StoreError> {
        let name = name.to_string();
        self.call(move |conn| {
            Ok(conn
                .query_row("SELECT v FROM kv WHERE k = ?1", params![name], |r| {
                    r.get::<_, String>(0)
                })
                .optional()?)
        })
        .await
    }

    async fn set_cursor(&self, name: &str, v: &str) -> Result<(), StoreError> {
        let name = name.to_string();
        let v = v.to_string();
        self.call(move |conn| {
            conn.execute(
                "INSERT INTO kv (k, v, updated_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(k) DO UPDATE SET v = excluded.v, updated_at = excluded.updated_at",
                params![name, v, now_ms()],
            )?;
            Ok(())
        })
        .await
    }

    async fn put_snapshot(&self, as_of: i64, payload: &str) -> Result<(), StoreError> {
        let payload = payload.to_string();
        self.call(move |conn| {
            conn.execute(
                "INSERT INTO snapshots (as_of, kind, payload) VALUES (?1, 'full', ?2)
                 ON CONFLICT(as_of) DO UPDATE SET payload = excluded.payload",
                params![as_of, payload],
            )?;
            Ok(())
        })
        .await
    }

    async fn last_snapshot(&self) -> Result<Option<(i64, String)>, StoreError> {
        self.call(|conn| {
            Ok(conn
                .query_row(
                    "SELECT as_of, payload FROM snapshots ORDER BY as_of DESC LIMIT 1",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?)
        })
        .await
    }

    async fn audit(&self, a: AuditEntry) -> Result<(), StoreError> {
        self.call(move |conn| {
            conn.execute(
                "INSERT INTO audit_log (chat_ref, actor, action, detail, at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![a.chat_ref, a.actor, a.action, a.detail, a.at],
            )?;
            Ok(())
        })
        .await
    }

    async fn audit_count(&self, chat_ref: &str) -> Result<i64, StoreError> {
        let chat_ref = chat_ref.to_string();
        self.call(move |conn| {
            let n: i64 = conn.query_row(
                "SELECT COUNT(*) FROM audit_log WHERE chat_ref = ?1",
                params![chat_ref],
                |r| r.get(0),
            )?;
            Ok(n)
        })
        .await
    }

    async fn purge_user(&self, user_id: i64) -> Result<crate::Purged, StoreError> {
        let secret = self.secret.clone();
        self.call(move |conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let target: Option<(i64, Option<String>)> = tx
                .query_row(
                    "SELECT chat_id, chat_ref FROM users WHERE user_id = ?1",
                    params![user_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let Some((chat_id, chat_ref)) = target else {
                tx.commit()?;
                return Ok(crate::Purged::default());
            };
            let chat_ref = chat_ref.unwrap_or_else(|| chat_ref_with(&secret, chat_id));
            let chat_ref_plain = chat_id.to_string();

            // Доставки не мають зовнішнього ключа, тому видаляються явно.
            let deliveries = tx.execute(
                "DELETE FROM deliveries WHERE chat_id = ?1",
                params![chat_id],
            )?;
            // Підписки та активи видаляються каскадом (foreign_keys = ON).
            let users = tx.execute("DELETE FROM users WHERE chat_id = ?1", params![chat_id])?;
            let audits = tx.execute(
                "DELETE FROM audit_log WHERE chat_ref = ?1 OR chat_ref = ?2",
                params![chat_ref, chat_ref_plain],
            )?;
            tx.commit()?;

            tracing::info!(
                user_id,
                chat_id,
                users = users as u32,
                deliveries = deliveries as u32,
                audits = audits as u32,
                "виконано GDPR-видалення користувача"
            );
            Ok(crate::Purged {
                users: users as u32,
                deliveries: deliveries as u32,
                audits: audits as u32,
            })
        })
        .await
    }
}
