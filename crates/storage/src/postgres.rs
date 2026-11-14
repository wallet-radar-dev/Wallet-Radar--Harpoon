//! Реалізація сховища на PostgreSQL.
//!
//! На відміну від SQLite, `sqlx` — асинхронний драйвер, тому жодна операція
//! не потребує `spawn_blocking`: запити виконуються прямо на async-екзекуторі.
//! З'єднання керуються пулом (`PgPool`), а не одним з'єднанням.
//!
//! Ключова відмінність від SQLite — `claim_deliveries`: там атомарність дає
//! транзакція з `BEGIN IMMEDIATE`, тут — `FOR UPDATE SKIP LOCKED`, тож
//! кілька воркерів можуть забирати чергу одночасно, не блокуючи один одного.
//!
//! Час у всіх полях — UTC, epoch-мілісекунди (`i64`), як і в SQLite-бекенді.

use std::sync::Arc;

use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::Row;

use crate::chat_ref::chat_ref_with;
use crate::{
    now_ms, AlertRecord, AuditEntry, Delivery, DeliveryStatus, DeliveryUpdate, FoldPending,
    Language, NewDelivery, NewUser, Priority, Role, Settings, Store, StoreError, Subscription,
    Timeframe, Topic, User, UserState,
};

/// Схема БД у PostgreSQL. Усі `CREATE ... IF NOT EXISTS` — повторний запуск
/// безпечний, тож міграцію можна застосувати на живой базі.
const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS users (
  chat_id BIGINT PRIMARY KEY,
  user_id BIGINT,
  username TEXT,
  role TEXT NOT NULL DEFAULT 'viewer',
  language TEXT NOT NULL DEFAULT 'uk',
  timezone TEXT NOT NULL DEFAULT 'UTC',
  timeframe TEXT NOT NULL DEFAULT '24h',
  muted BOOLEAN NOT NULL DEFAULT FALSE,
  blocked BOOLEAN NOT NULL DEFAULT FALSE,
  state TEXT NOT NULL DEFAULT 'active',
  chat_ref TEXT,
  created_at BIGINT NOT NULL,
  updated_at BIGINT NOT NULL,
  last_seen_at BIGINT
);
CREATE UNIQUE INDEX IF NOT EXISTS users_user_id_idx ON users(user_id) WHERE user_id IS NOT NULL;
CREATE TABLE IF NOT EXISTS subscriptions (
  chat_id BIGINT NOT NULL REFERENCES users(chat_id) ON DELETE CASCADE,
  topic TEXT NOT NULL,
  min_severity TEXT NOT NULL DEFAULT 'medium',
  PRIMARY KEY (chat_id, topic)
);
CREATE TABLE IF NOT EXISTS user_assets (
  chat_id BIGINT NOT NULL REFERENCES users(chat_id) ON DELETE CASCADE,
  symbol TEXT NOT NULL,
  PRIMARY KEY (chat_id, symbol)
);
CREATE TABLE IF NOT EXISTS alert_log (
  alert_key TEXT PRIMARY KEY,
  kind TEXT NOT NULL,
  asset TEXT,
  window_start BIGINT NOT NULL,
  severity TEXT NOT NULL,
  degraded BOOLEAN NOT NULL DEFAULT FALSE,
  payload TEXT,
  created_at BIGINT NOT NULL
);
CREATE INDEX IF NOT EXISTS alert_log_time_idx ON alert_log (created_at);
CREATE INDEX IF NOT EXISTS alert_log_asset_idx ON alert_log (asset, created_at);
CREATE TABLE IF NOT EXISTS deliveries (
  chat_id BIGINT NOT NULL,
  alert_key TEXT NOT NULL,
  priority SMALLINT NOT NULL,
  status TEXT NOT NULL,
  attempts INTEGER NOT NULL DEFAULT 0,
  next_attempt_at BIGINT NOT NULL,
  created_at BIGINT NOT NULL,
  sent_at BIGINT,
  last_error TEXT,
  text TEXT NOT NULL DEFAULT '',
  PRIMARY KEY (chat_id, alert_key)
);
CREATE INDEX IF NOT EXISTS deliveries_due_idx ON deliveries (status, next_attempt_at, priority);
CREATE INDEX IF NOT EXISTS deliveries_backlog_idx ON deliveries (status, created_at);
CREATE TABLE IF NOT EXISTS seen_events (event_id TEXT PRIMARY KEY, ts BIGINT NOT NULL);
CREATE INDEX IF NOT EXISTS seen_events_ts_idx ON seen_events (ts);
CREATE TABLE IF NOT EXISTS seen_updates (update_id BIGINT PRIMARY KEY, seen_at BIGINT NOT NULL);
CREATE TABLE IF NOT EXISTS fold_pending (
  alert_key TEXT PRIMARY KEY,
  kind TEXT NOT NULL,
  win TEXT NOT NULL,
  asset TEXT,
  payload TEXT NOT NULL,
  created_at BIGINT NOT NULL
);
CREATE INDEX IF NOT EXISTS fold_pending_time_idx ON fold_pending (created_at);
CREATE TABLE IF NOT EXISTS snapshots (
  as_of BIGINT PRIMARY KEY,
  kind TEXT NOT NULL DEFAULT 'full',
  payload TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS audit_log (
  id BIGSERIAL PRIMARY KEY,
  chat_ref TEXT NOT NULL,
  actor TEXT NOT NULL,
  action TEXT NOT NULL,
  detail TEXT,
  at BIGINT NOT NULL
);
CREATE INDEX IF NOT EXISTS audit_log_ref_idx ON audit_log (chat_ref, at);
CREATE TABLE IF NOT EXISTS kv (k TEXT PRIMARY KEY, v TEXT NOT NULL, updated_at BIGINT NOT NULL);
"#;

/// Сховище стану на PostgreSQL.
#[derive(Clone)]
pub struct PostgresStore {
    pool: PgPool,
    /// Секрет для підписування `chat_ref`.
    secret: Arc<Vec<u8>>,
    /// Тривалість оренди (lease) доставки: скільки часу рядок у статусі
    /// `sending` вважається «в роботі». Після цієї межі `sending`-рядок
    /// повертається в чергу (див. `expire_stuck_sending_before`).
    lease_ms: i64,
}

/// Типова тривалість оренди доставки: 5 хвилин.
pub const DEFAULT_DELIVERY_LEASE_MS: i64 = 300_000;

impl PostgresStore {
    /// Підключається до бази й застосовує схему.
    ///
    /// `secret` — ключ HMAC для `chat_ref`; порожній означає підписування без
    /// ключа (простий хеш). `max_connections` — розмір пулу з'єднань.
    pub async fn connect(
        url: &str,
        secret: &[u8],
        max_connections: u32,
    ) -> Result<Self, StoreError> {
        Self::connect_with(url, secret, max_connections, 10).await
    }

    /// Підключається до бази із таймаутом на встановлення з'єднання.
    pub async fn connect_with(
        url: &str,
        secret: &[u8],
        max_connections: u32,
        connect_timeout_secs: u64,
    ) -> Result<Self, StoreError> {
        let pool = PgPoolOptions::new()
            .max_connections(max_connections.max(1))
            .acquire_timeout(std::time::Duration::from_secs(connect_timeout_secs.max(1)))
            .connect(url)
            .await?;
        let store = PostgresStore {
            pool,
            secret: Arc::new(secret.to_vec()),
            lease_ms: DEFAULT_DELIVERY_LEASE_MS,
        };
        store.migrate().await?;
        Ok(store)
    }

    /// Сховище поверх уже готового пулу (наприклад, з налаштованим
    /// `search_path` — так тести ізольовані схемами).
    pub fn from_pool(pool: PgPool, secret: &[u8]) -> Self {
        PostgresStore {
            pool,
            secret: Arc::new(secret.to_vec()),
            lease_ms: DEFAULT_DELIVERY_LEASE_MS,
        }
    }

    /// Тривалість оренди доставки (lease), мс.
    ///
    /// Скільки часу доставка у статусі `sending` вважається «в роботі».
    /// Після цієї межі `sending`-рядок повертається в чергу — інакше
    /// падіння процесу між claim і finish назавжди «залипало» б доставку.
    pub fn with_lease(mut self, lease_ms: i64) -> Self {
        self.lease_ms = lease_ms.max(1_000);
        self
    }

    /// Застосовує схему (DDL). Безпечно викликати повторно.
    pub async fn migrate(&self) -> Result<(), StoreError> {
        for stmt in SCHEMA.split(';') {
            let stmt = stmt.trim();
            if stmt.is_empty() {
                continue;
            }
            sqlx::query(stmt).execute(&self.pool).await?;
        }
        Ok(())
    }

    /// Перевіряє, що база жива (`SELECT 1`).
    pub async fn ping(&self) -> Result<(), StoreError> {
        sqlx::query("SELECT 1::bigint").execute(&self.pool).await?;
        Ok(())
    }

    /// Пул з'єднань — для мігратора та службових запитів.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Псевдонім чату для аудиту й логів.
    pub fn chat_ref(&self, chat_id: i64) -> String {
        chat_ref_with(&self.secret, chat_id)
    }
}

/// Читає колонки користувача у вказаному порядку.
const USER_COLS: &str = "chat_id, user_id, username, role, language, timezone, timeframe, muted, \
blocked, state, created_at, updated_at, last_seen_at";

fn row_to_user(row: &sqlx::postgres::PgRow) -> Result<User, sqlx::Error> {
    let role: String = row.try_get("role")?;
    let language: String = row.try_get("language")?;
    let timeframe: String = row.try_get("timeframe")?;
    let state: String = row.try_get("state")?;
    Ok(User {
        chat_id: row.try_get("chat_id")?,
        user_id: row.try_get("user_id")?,
        username: row.try_get("username")?,
        role: Role::from_str_code(&role),
        language: Language::from_str_code(&language),
        timezone: row.try_get("timezone")?,
        timeframe: Timeframe::from_str_code(&timeframe),
        muted: row.try_get("muted")?,
        blocked: row.try_get("blocked")?,
        state: UserState::from_str_code(&state),
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        last_seen_at: row.try_get("last_seen_at")?,
    })
}

fn row_to_delivery(row: &sqlx::postgres::PgRow) -> Result<Delivery, sqlx::Error> {
    let priority: i16 = row.try_get("priority")?;
    let status: String = row.try_get("status")?;
    let attempts: i32 = row.try_get("attempts")?;
    Ok(Delivery {
        chat_id: row.try_get("chat_id")?,
        alert_key: row.try_get("alert_key")?,
        priority: Priority::from_i64(i64::from(priority)),
        text: row.try_get::<Option<String>, _>("text")?.unwrap_or_default(),
        status: DeliveryStatus::from_str_code(&status),
        attempts: attempts.max(0) as u32,
        next_attempt_at: row.try_get("next_attempt_at")?,
        created_at: row.try_get("created_at")?,
    })
}

impl Store for PostgresStore {
    fn chat_ref(&self, chat_id: i64) -> String {
        PostgresStore::chat_ref(self, chat_id)
    }

    async fn upsert_user(&self, u: NewUser) -> Result<User, StoreError> {
        let chat_ref = PostgresStore::chat_ref(self, u.chat_id);
        let now = now_ms();
        let sql = format!(
            "INSERT INTO users (chat_id, user_id, username, role, language, timezone, chat_ref, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $8)
             ON CONFLICT (chat_id) DO UPDATE SET
               user_id = COALESCE(EXCLUDED.user_id, users.user_id),
               username = EXCLUDED.username,
               role = EXCLUDED.role,
               language = EXCLUDED.language,
               timezone = EXCLUDED.timezone,
               chat_ref = COALESCE(users.chat_ref, EXCLUDED.chat_ref),
               updated_at = EXCLUDED.updated_at
             RETURNING {USER_COLS}"
        );
        let row = sqlx::query(&sql)
            .bind(u.chat_id)
            .bind(u.user_id)
            .bind(u.username.as_deref())
            .bind(u.role.as_str())
            .bind(u.language.as_str())
            .bind(&u.timezone)
            .bind(&chat_ref)
            .bind(now)
            .fetch_one(&self.pool)
            .await?;
        row_to_user(&row).map_err(StoreError::from)
    }

    async fn get_user(&self, chat_id: i64) -> Result<Option<User>, StoreError> {
        let sql = format!("SELECT {USER_COLS} FROM users WHERE chat_id = $1");
        let row = sqlx::query(&sql)
            .bind(chat_id)
            .fetch_optional(&self.pool)
            .await?;
        row.map(|r| row_to_user(&r).map_err(StoreError::from))
            .transpose()
    }

    async fn find_user_by_user_id(&self, user_id: i64) -> Result<Option<User>, StoreError> {
        let sql = format!("SELECT {USER_COLS} FROM users WHERE user_id = $1");
        let row = sqlx::query(&sql)
            .bind(user_id)
            .fetch_optional(&self.pool)
            .await?;
        row.map(|r| row_to_user(&r).map_err(StoreError::from))
            .transpose()
    }

    async fn set_settings(&self, chat_id: i64, s: &Settings) -> Result<(), StoreError> {
        let changed = sqlx::query(
            "UPDATE users SET language = $2, timezone = $3, timeframe = $4, muted = $5, updated_at = $6
             WHERE chat_id = $1",
        )
        .bind(chat_id)
        .bind(s.language.as_str())
        .bind(&s.timezone)
        .bind(s.timeframe.as_str())
        .bind(s.muted)
        .bind(now_ms())
        .execute(&self.pool)
        .await?
        .rows_affected();
        if changed == 0 {
            return Err(StoreError::NotFound(chat_id));
        }
        Ok(())
    }

    async fn set_last_seen(&self, chat_id: i64, at: i64) -> Result<(), StoreError> {
        let changed = sqlx::query(
            "UPDATE users SET last_seen_at = $2, updated_at = $2 WHERE chat_id = $1",
        )
        .bind(chat_id)
        .bind(at)
        .execute(&self.pool)
        .await?
        .rows_affected();
        if changed == 0 {
            return Err(StoreError::NotFound(chat_id));
        }
        Ok(())
    }

    async fn set_blocked(&self, chat_id: i64, blocked: bool) -> Result<(), StoreError> {
        let changed = sqlx::query("UPDATE users SET blocked = $2, updated_at = $3 WHERE chat_id = $1")
            .bind(chat_id)
            .bind(blocked)
            .bind(now_ms())
            .execute(&self.pool)
            .await?
            .rows_affected();
        if changed == 0 {
            return Err(StoreError::NotFound(chat_id));
        }
        Ok(())
    }

    async fn subscriptions(&self, chat_id: i64) -> Result<Vec<Subscription>, StoreError> {
        let rows = sqlx::query(
            "SELECT topic, min_severity FROM subscriptions WHERE chat_id = $1 ORDER BY topic",
        )
        .bind(chat_id)
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let topic: String = row.try_get("topic")?;
            out.push(Subscription {
                chat_id,
                topic: Topic::from_str_code(&topic),
                min_severity: row.try_get("min_severity")?,
            });
        }
        Ok(out)
    }

    async fn set_subscription(&self, s: &Subscription) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO subscriptions (chat_id, topic, min_severity) VALUES ($1, $2, $3)
             ON CONFLICT (chat_id, topic) DO UPDATE SET min_severity = EXCLUDED.min_severity",
        )
        .bind(s.chat_id)
        .bind(s.topic.as_str())
        .bind(&s.min_severity)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn remove_subscription(&self, chat_id: i64, topic: Topic) -> Result<(), StoreError> {
        sqlx::query("DELETE FROM subscriptions WHERE chat_id = $1 AND topic = $2")
            .bind(chat_id)
            .bind(topic.as_str())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn user_assets(&self, chat_id: i64) -> Result<Vec<String>, StoreError> {
        let rows = sqlx::query("SELECT symbol FROM user_assets WHERE chat_id = $1 ORDER BY symbol")
            .bind(chat_id)
            .fetch_all(&self.pool)
            .await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            out.push(row.try_get("symbol")?);
        }
        Ok(out)
    }

    async fn set_user_asset(&self, chat_id: i64, symbol: &str, on: bool) -> Result<(), StoreError> {
        let symbol = symbol.trim().to_ascii_uppercase();
        if on {
            sqlx::query(
                "INSERT INTO user_assets (chat_id, symbol) VALUES ($1, $2)
                 ON CONFLICT (chat_id, symbol) DO NOTHING",
            )
            .bind(chat_id)
            .bind(&symbol)
            .execute(&self.pool)
            .await?;
        } else {
            sqlx::query("DELETE FROM user_assets WHERE chat_id = $1 AND symbol = $2")
                .bind(chat_id)
                .bind(&symbol)
                .execute(&self.pool)
                .await?;
        }
        Ok(())
    }

    async fn register_alert(&self, a: &AlertRecord) -> Result<bool, StoreError> {
        let changed = sqlx::query(
            "INSERT INTO alert_log (alert_key, kind, asset, window_start, severity, degraded, payload, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
             ON CONFLICT (alert_key) DO NOTHING",
        )
        .bind(&a.alert_key)
        .bind(&a.kind)
        .bind(a.asset.as_deref())
        .bind(a.window_start)
        .bind(&a.severity)
        .bind(a.degraded)
        .bind(&a.payload)
        .bind(a.created_at)
        .execute(&self.pool)
        .await?
        .rows_affected();
        Ok(changed > 0)
    }

    async fn alert_exists(&self, alert_key: &str) -> Result<bool, StoreError> {
        let found: Option<(i64,)> =
            sqlx::query_as("SELECT 1::bigint FROM alert_log WHERE alert_key = $1")
            .bind(alert_key)
            .fetch_optional(&self.pool)
            .await?;
        Ok(found.is_some())
    }

    async fn enqueue_delivery(&self, d: NewDelivery) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO deliveries
               (chat_id, alert_key, priority, status, attempts, next_attempt_at, created_at, text)
             VALUES ($1, $2, $3, 'pending', 0, $4, $5, $6)
             ON CONFLICT (chat_id, alert_key) DO NOTHING",
        )
        .bind(d.chat_id)
        .bind(&d.alert_key)
        .bind(d.priority.as_i64() as i16)
        .bind(d.created_at)
        .bind(d.created_at)
        .bind(&d.text)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn claim_deliveries(&self, now: i64, limit: u32) -> Result<Vec<Delivery>, StoreError> {
        // Один запит: спершу відбираємо рядки `FOR UPDATE SKIP LOCKED` (тож
        // інший воркер не бере ті самі), потім переводимо їх у `sending` і
        // повертаємо вже оновлені рядки.
        let rows = sqlx::query(
            "WITH claimed AS (
               SELECT chat_id, alert_key FROM deliveries
               WHERE status = 'pending' AND next_attempt_at <= $1
               ORDER BY priority ASC, next_attempt_at ASC
               LIMIT $2
               FOR UPDATE SKIP LOCKED
             )
             UPDATE deliveries d SET status = 'sending', next_attempt_at = $1 + $3
             FROM claimed c
             WHERE d.chat_id = c.chat_id AND d.alert_key = c.alert_key
             RETURNING d.chat_id, d.alert_key, d.priority, d.status, d.attempts,
                       d.next_attempt_at, d.created_at, d.text",
        )
        .bind(now)
        .bind(i64::from(limit))
        .bind(self.lease_ms)
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            out.push(row_to_delivery(&row)?);
        }
        Ok(out)
    }

    async fn finish_delivery(&self, d: DeliveryUpdate) -> Result<(), StoreError> {
        // `sent_at` заповнюється лише для фінального успіху; для повтору
        // лишається NULL, щоб було видно незавершені доставки.
        sqlx::query(
            "UPDATE deliveries SET status = $3, attempts = $4, next_attempt_at = $5, last_error = $6,
               sent_at = CASE WHEN $3 = 'sent' THEN $7 ELSE sent_at END
             WHERE chat_id = $1 AND alert_key = $2",
        )
        .bind(d.chat_id)
        .bind(&d.alert_key)
        .bind(d.status.as_str())
        .bind(i64::from(d.attempts))
        .bind(d.next_attempt_at)
        .bind(d.error.as_deref())
        .bind(now_ms())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn expire_pending_before(
        &self,
        cutoff_ms: i64,
        reason: &str,
    ) -> Result<usize, StoreError> {
        // Тільки `pending`: решта статусів — уже завершені спроби.
        let n = sqlx::query(
            "UPDATE deliveries
             SET status = 'skipped', next_attempt_at = $1, last_error = $2
             WHERE status = 'pending' AND created_at < $3",
        )
        .bind(cutoff_ms)
        .bind(reason)
        .bind(cutoff_ms)
        .execute(&self.pool)
        .await?
        .rows_affected();
        Ok(n as usize)
    }

    async fn expire_stuck_sending_before(
        &self,
        cutoff_ms: i64,
        reason: &str,
    ) -> Result<usize, StoreError> {
        // Тільки `sending`, у яких минув дедлайн оренди: вони лишилися
        // від воркера, який не зафіксував результат. Повертаємо їх у
        // `pending` — воркер забере їх знову.
        let n = sqlx::query(
            "UPDATE deliveries
             SET status = 'pending', next_attempt_at = $1, attempts = attempts + 1, last_error = $2
             WHERE status = 'sending' AND next_attempt_at <= $3",
        )
        .bind(now_ms())
        .bind(reason)
        .bind(cutoff_ms)
        .execute(&self.pool)
        .await?
        .rows_affected();
        Ok(n as usize)
    }

    async fn count_pending(&self) -> Result<i64, StoreError> {
        let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM deliveries WHERE status = 'pending'")
            .fetch_one(&self.pool)
            .await?;
        Ok(n)
    }

    async fn recipients_for(&self, topic: Topic) -> Result<Vec<i64>, StoreError> {
        let rows = sqlx::query(
            "SELECT DISTINCT u.chat_id FROM users u
             JOIN subscriptions s ON s.chat_id = u.chat_id
             WHERE s.topic = $1 AND u.muted = FALSE AND u.blocked = FALSE AND u.state = 'active'
             ORDER BY u.chat_id",
        )
        .bind(topic.as_str())
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            out.push(row.try_get("chat_id")?);
        }
        Ok(out)
    }

    async fn count_recipients(&self) -> Result<i64, StoreError> {
        let (n,): (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM users WHERE muted = FALSE AND blocked = FALSE AND state = 'active'",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(n)
    }

    async fn all_active_chats(&self) -> Result<Vec<i64>, StoreError> {
        let rows = sqlx::query(
            "SELECT chat_id FROM users
             WHERE muted = FALSE AND blocked = FALSE AND state = 'active'
             ORDER BY chat_id",
        )
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            out.push(row.try_get("chat_id")?);
        }
        Ok(out)
    }

    async fn list_users(&self) -> Result<Vec<User>, StoreError> {
        let sql = format!("SELECT {USER_COLS} FROM users ORDER BY chat_id");
        let rows = sqlx::query(&sql).fetch_all(&self.pool).await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            out.push(row_to_user(&row)?);
        }
        Ok(out)
    }

    async fn alerts_since(&self, since_ms: i64) -> Result<Vec<AlertRecord>, StoreError> {
        let rows = sqlx::query(
            "SELECT alert_key, kind, asset, window_start, severity, degraded, payload, created_at
             FROM alert_log WHERE created_at >= $1 ORDER BY created_at DESC, alert_key",
        )
        .bind(since_ms)
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            out.push(AlertRecord {
                alert_key: row.try_get("alert_key")?,
                kind: row.try_get("kind")?,
                asset: row.try_get("asset")?,
                window_start: row.try_get("window_start")?,
                severity: row.try_get("severity")?,
                degraded: row.try_get("degraded")?,
                payload: row.try_get::<Option<String>, _>("payload")?.unwrap_or_default(),
                created_at: row.try_get("created_at")?,
            });
        }
        Ok(out)
    }

    async fn fold_pending_insert(&self, p: &FoldPending) -> Result<bool, StoreError> {
        let changed = sqlx::query(
            "INSERT INTO fold_pending (alert_key, kind, win, asset, payload, created_at)
             VALUES ($1, $2, $3, $4, $5, $6)
             ON CONFLICT (alert_key) DO NOTHING",
        )
        .bind(&p.alert_key)
        .bind(&p.kind)
        .bind(&p.win)
        .bind(p.asset.as_deref())
        .bind(&p.payload)
        .bind(p.created_at)
        .execute(&self.pool)
        .await?
        .rows_affected();
        Ok(changed > 0)
    }

    async fn fold_pending_list(&self) -> Result<Vec<FoldPending>, StoreError> {
        let rows = sqlx::query(
            "SELECT alert_key, kind, win, asset, payload, created_at FROM fold_pending
             ORDER BY created_at",
        )
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            out.push(FoldPending {
                alert_key: row.try_get("alert_key")?,
                kind: row.try_get("kind")?,
                win: row.try_get("win")?,
                asset: row.try_get("asset")?,
                payload: row.try_get("payload")?,
                created_at: row.try_get("created_at")?,
            });
        }
        Ok(out)
    }

    async fn fold_pending_remove(&self, alert_keys: &[String]) -> Result<usize, StoreError> {
        if alert_keys.is_empty() {
            return Ok(0);
        }
        let n = sqlx::query("DELETE FROM fold_pending WHERE alert_key = ANY($1)")
            .bind(alert_keys)
            .execute(&self.pool)
            .await?
            .rows_affected();
        Ok(n as usize)
    }

    async fn seen_event(&self, event_id: &str, ts: i64) -> Result<bool, StoreError> {
        let changed = sqlx::query(
            "INSERT INTO seen_events (event_id, ts) VALUES ($1, $2)
             ON CONFLICT (event_id) DO NOTHING",
        )
        .bind(event_id)
        .bind(ts)
        .execute(&self.pool)
        .await?
        .rows_affected();
        Ok(changed > 0)
    }

    async fn seen_update(&self, update_id: i64) -> Result<bool, StoreError> {
        let changed = sqlx::query(
            "INSERT INTO seen_updates (update_id, seen_at) VALUES ($1, $2)
             ON CONFLICT (update_id) DO NOTHING",
        )
        .bind(update_id)
        .bind(now_ms())
        .execute(&self.pool)
        .await?
        .rows_affected();
        Ok(changed > 0)
    }

    async fn cursor(&self, name: &str) -> Result<Option<String>, StoreError> {
        let row = sqlx::query("SELECT v FROM kv WHERE k = $1")
            .bind(name)
            .fetch_optional(&self.pool)
            .await?;
        match row {
            None => Ok(None),
            Some(r) => Ok(Some(r.try_get("v")?)),
        }
    }

    async fn set_cursor(&self, name: &str, v: &str) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO kv (k, v, updated_at) VALUES ($1, $2, $3)
             ON CONFLICT (k) DO UPDATE SET v = EXCLUDED.v, updated_at = EXCLUDED.updated_at",
        )
        .bind(name)
        .bind(v)
        .bind(now_ms())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn put_snapshot(&self, as_of: i64, payload: &str) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO snapshots (as_of, kind, payload) VALUES ($1, 'full', $2)
             ON CONFLICT (as_of) DO UPDATE SET payload = EXCLUDED.payload",
        )
        .bind(as_of)
        .bind(payload)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn last_snapshot(&self) -> Result<Option<(i64, String)>, StoreError> {
        let row = sqlx::query("SELECT as_of, payload FROM snapshots ORDER BY as_of DESC LIMIT 1")
            .fetch_optional(&self.pool)
            .await?;
        match row {
            None => Ok(None),
            Some(r) => Ok(Some((r.try_get("as_of")?, r.try_get("payload")?))),
        }
    }

    async fn audit(&self, a: AuditEntry) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO audit_log (chat_ref, actor, action, detail, at) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(&a.chat_ref)
        .bind(&a.actor)
        .bind(&a.action)
        .bind(a.detail.as_deref())
        .bind(a.at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn audit_count(&self, chat_ref: &str) -> Result<i64, StoreError> {
        let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM audit_log WHERE chat_ref = $1")
            .bind(chat_ref)
            .fetch_one(&self.pool)
            .await?;
        Ok(n)
    }

    async fn purge_user(&self, user_id: i64) -> Result<crate::Purged, StoreError> {
        let secret = self.secret.clone();
        let mut tx = self.pool.begin().await?;
        let target: Option<(i64, Option<String>)> =
            sqlx::query_as("SELECT chat_id, chat_ref FROM users WHERE user_id = $1")
                .bind(user_id)
                .fetch_optional(&mut *tx)
                .await?;
        let Some((chat_id, chat_ref)) = target else {
            tx.commit().await?;
            return Ok(crate::Purged::default());
        };
        let chat_ref = chat_ref.unwrap_or_else(|| chat_ref_with(&secret, chat_id));
        let chat_ref_plain = chat_id.to_string();

        // Доставки не мають зовнішнього ключа, тому видаляються явно.
        let deliveries = sqlx::query("DELETE FROM deliveries WHERE chat_id = $1")
            .bind(chat_id)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        // Підписки та активи видаляються каскадом (foreign_keys = ON).
        let users = sqlx::query("DELETE FROM users WHERE chat_id = $1")
            .bind(chat_id)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        let audits = sqlx::query("DELETE FROM audit_log WHERE chat_ref = $1 OR chat_ref = $2")
            .bind(&chat_ref)
            .bind(&chat_ref_plain)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        tx.commit().await?;

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
    }
}
