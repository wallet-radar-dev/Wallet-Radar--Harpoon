//! Перенесення стану бота з SQLite у PostgreSQL.
//!
//! Інструмент читає таблиці SQLite-бази бота і переносить їх у Postgres
//! **в одній транзакції**: якщо перенесення обривається, Postgres лишається
//! без змін — немає «напівперенесеної» бази.
//!
//! Повторний запуск безпечний: усі вставки — `ON CONFLICT DO NOTHING`,
//! тож уже перенесені рядки не дублюються.
//!
//! Використання:
//!
//! ```text
//! migrate-sqlite-to-postgres --sqlite data/bot.db --postgres-url postgres://user:pass@127.0.0.1:5432/wallet_radar
//! ```
//!
//! або через змінну оточення `POSTGRES_URL` (без `--postgres-url`).

use anyhow::{Context, Result};
use rusqlite::Connection;
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row, Transaction};

/// Значення одного стовпця — тип приводиться до типу Postgres.
#[derive(Clone, Debug)]
enum Val {
    Int(i64),
    Text(String),
    OptText(Option<String>),
    OptInt(Option<i64>),
    Bool(bool),
}

/// Таблиці в порядку перенесення (спочатку — ті, від яких залежать інші).
const TABLES: &[&str] = &[
    "users",
    "subscriptions",
    "user_assets",
    "alert_log",
    "deliveries",
    "seen_events",
    "seen_updates",
    "fold_pending",
    "snapshots",
    "audit_log",
    "kv",
];

#[tokio::main]
async fn main() -> Result<()> {
    let mut sqlite_path = String::new();
    let mut pg_url = std::env::var("POSTGRES_URL").unwrap_or_default();

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--sqlite" => sqlite_path = args.next().unwrap_or_default(),
            "--postgres-url" => pg_url = args.next().unwrap_or_default(),
            "--help" | "-h" => {
                println!(
                    "використання: migrate-sqlite-to-postgres --sqlite <шлях> [--postgres-url <url>]\n\
                     URL також читається зі змінної оточення POSTGRES_URL"
                );
                return Ok(());
            }
            other => anyhow::bail!("невідомий аргумент: {other}"),
        }
    }

    if sqlite_path.is_empty() {
        anyhow::bail!("потрібен --sqlite <шлях до файлу SQLite>");
    }
    if pg_url.is_empty() {
        anyhow::bail!("потрібен --postgres-url <url> або змінна POSTGRES_URL");
    }

    let conn = Connection::open_with_flags(&sqlite_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("відкриваю SQLite {sqlite_path} на читання"))?;

    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&pg_url)
        .await
        .with_context(|| format!("підключаюся до Postgres {}", redact(&pg_url)))?;

    // Схема застосовується до перенесення — інакше вставки падатимуть.
    let store = storage::PostgresStore::connect(&pg_url, &[], 2)
        .await
        .with_context(|| "застосовую схему Postgres")?;
    drop(store);

    println!("переношу {sqlite_path} → Postgres");
    let mut tx = pool.begin().await.context("починаю транзакцію")?;

    for table in TABLES {
        let n = copy_table(&conn, &mut tx, table).await?;
        println!("  {table}: {n} рядків");
    }

    // Послідовність audit_log.id має продовжуватися після явних id.
    sqlx::query(
        "SELECT setval(pg_get_serial_sequence('audit_log', 'id'),
                       COALESCE((SELECT MAX(id) FROM audit_log), 1))",
    )
    .execute(&mut *tx)
    .await
    .context("синхронізую послідовність audit_log")?;

    tx.commit().await.context("комічу транзакцію")?;
    println!("перенесення завершено");
    Ok(())
}

/// Копіює одну таблицю: читає рядки з SQLite і вставляє їх у Postgres.
///
/// Схема відповідності (стовпці SQLite → стовпці Postgres) задана тут же —
/// щоб мігратор і `PostgresStore` читали ту саму базу однаково.
async fn copy_table(
    conn: &Connection,
    tx: &mut Transaction<'_, sqlx::Postgres>,
    table: &str,
) -> Result<usize> {
    if !table_exists(conn, table)? {
        println!("  {table}: таблиці немає у SQLite — пропускаю");
        return Ok(0);
    }

    // У старих базах колонки `chat_ref` немає — тоді беремо лише те, що є.
    let has_chat_ref = table == "users"
        && conn
            .prepare("PRAGMA table_info(users)")?
            .query_map([], |r| r.get::<_, String>(1))?
            .collect::<rusqlite::Result<Vec<String>>>()?
            .iter()
            .any(|c| c == "chat_ref");

    let select = match table {
        "users" => {
            if has_chat_ref {
                "SELECT chat_id, user_id, username, role, language, timezone, timeframe, muted,
                        blocked, state, chat_ref, created_at, updated_at, last_seen_at FROM users"
            } else {
                "SELECT chat_id, user_id, username, role, language, timezone, timeframe, muted,
                        blocked, state, NULL, created_at, updated_at, last_seen_at FROM users"
            }
        }
        "subscriptions" => "SELECT chat_id, topic, min_severity FROM subscriptions",
        "user_assets" => "SELECT chat_id, symbol FROM user_assets",
        "alert_log" => "SELECT alert_key, kind, asset, window_start, severity, degraded, payload,
                        created_at FROM alert_log",
        "deliveries" => "SELECT chat_id, alert_key, priority, status, attempts, next_attempt_at,
                         created_at, sent_at, last_error, text FROM deliveries",
        "seen_events" => "SELECT event_id, ts FROM seen_events",
        "seen_updates" => "SELECT update_id, seen_at FROM seen_updates",
        "fold_pending" => "SELECT alert_key, kind, win, asset, payload, created_at FROM fold_pending",
        "snapshots" => "SELECT as_of, kind, payload FROM snapshots",
        "audit_log" => "SELECT id, chat_ref, actor, action, detail, at FROM audit_log ORDER BY id",
        "kv" => "SELECT k, v, updated_at FROM kv",
        _ => return Ok(0),
    };

    let (pg_table, cols, conflict) = match table {
        "users" => (
            "users",
            &[
                "chat_id",
                "user_id",
                "username",
                "role",
                "language",
                "timezone",
                "timeframe",
                "muted",
                "blocked",
                "state",
                "chat_ref",
                "created_at",
                "updated_at",
                "last_seen_at",
            ][..],
            &["chat_id"][..],
        ),
        "subscriptions" => (
            "subscriptions",
            &["chat_id", "topic", "min_severity"][..],
            &["chat_id", "topic"][..],
        ),
        "user_assets" => ("user_assets", &["chat_id", "symbol"][..], &["chat_id", "symbol"][..]),
        "alert_log" => (
            "alert_log",
            &[
                "alert_key",
                "kind",
                "asset",
                "window_start",
                "severity",
                "degraded",
                "payload",
                "created_at",
            ][..],
            &["alert_key"][..],
        ),
        "deliveries" => (
            "deliveries",
            &[
                "chat_id",
                "alert_key",
                "priority",
                "status",
                "attempts",
                "next_attempt_at",
                "created_at",
                "sent_at",
                "last_error",
                "text",
            ][..],
            &["chat_id", "alert_key"][..],
        ),
        "seen_events" => ("seen_events", &["event_id", "ts"][..], &["event_id"][..]),
        "seen_updates" => ("seen_updates", &["update_id", "seen_at"][..], &["update_id"][..]),
        "fold_pending" => (
            "fold_pending",
            &["alert_key", "kind", "win", "asset", "payload", "created_at"][..],
            &["alert_key"][..],
        ),
        "snapshots" => (
            "snapshots",
            &["as_of", "kind", "payload"][..],
            &["as_of"][..],
        ),
        "audit_log" => (
            "audit_log",
            &["id", "chat_ref", "actor", "action", "detail", "at"][..],
            &["id"][..],
        ),
        "kv" => ("kv", &["k", "v", "updated_at"][..], &["k"][..]),
        _ => return Ok(0),
    };

    let placeholders: Vec<String> = (1..=cols.len()).map(|i| format!("${i}")).collect();
    let sql = format!(
        "INSERT INTO {pg_table} ({}) VALUES ({}) ON CONFLICT ({}) DO NOTHING",
        cols.join(", "),
        placeholders.join(", "),
        conflict.join(", ")
    );

    let mut stmt = conn.prepare(select)?;
    let mut rows = stmt.query([])?;
    let mut count = 0usize;
    while let Some(row) = rows.next()? {
        let values = row_to_vals(&row, table)?;
        let mut query = sqlx::query(&sql);
        for v in values {
            query = match v {
                Val::Int(i) => query.bind(i),
                Val::Text(s) => query.bind(s),
                Val::OptText(s) => query.bind(s),
                Val::OptInt(i) => query.bind(i),
                Val::Bool(b) => query.bind(b),
            };
        }
        query.execute(&mut **tx).await?;
        count += 1;
    }
    Ok(count)
}

/// Перетворює рядок SQLite на типізований набір значень для Postgres.
fn row_to_vals(row: &rusqlite::Row<'_>, table: &str) -> Result<Vec<Val>> {
    let vals = match table {
        "users" => vec![
            Val::Int(row.get(0)?),
            Val::OptInt(row.get(1)?),
            Val::OptText(row.get(2)?),
            Val::Text(row.get(3)?),
            Val::Text(row.get(4)?),
            Val::Text(row.get(5)?),
            Val::Text(row.get(6)?),
            Val::Bool(row.get::<_, i64>(7)? != 0),
            Val::Bool(row.get::<_, i64>(8)? != 0),
            Val::Text(row.get(9)?),
            Val::OptText(row.get(10)?),
            Val::Int(row.get(11)?),
            Val::Int(row.get(12)?),
            Val::OptInt(row.get(13)?),
        ],
        "subscriptions" => vec![
            Val::Int(row.get(0)?),
            Val::Text(row.get(1)?),
            Val::Text(row.get(2)?),
        ],
        "user_assets" => vec![Val::Int(row.get(0)?), Val::Text(row.get(1)?)],
        "alert_log" => vec![
            Val::Text(row.get(0)?),
            Val::Text(row.get(1)?),
            Val::OptText(row.get(2)?),
            Val::Int(row.get(3)?),
            Val::Text(row.get(4)?),
            Val::Bool(row.get::<_, i64>(5)? != 0),
            Val::OptText(row.get(6)?),
            Val::Int(row.get(7)?),
        ],
        "deliveries" => vec![
            Val::Int(row.get(0)?),
            Val::Text(row.get(1)?),
            Val::Int(row.get::<_, i64>(2)?),
            Val::Text(row.get(3)?),
            Val::Int(row.get::<_, i64>(4)?),
            Val::Int(row.get(5)?),
            Val::Int(row.get(6)?),
            Val::OptInt(row.get(7)?),
            Val::OptText(row.get(8)?),
            Val::OptText(row.get(9)?),
        ],
        "seen_events" => vec![Val::Text(row.get(0)?), Val::Int(row.get(1)?)],
        "seen_updates" => vec![Val::Int(row.get(0)?), Val::Int(row.get(1)?)],
        "fold_pending" => vec![
            Val::Text(row.get(0)?),
            Val::Text(row.get(1)?),
            Val::Text(row.get(2)?),
            Val::OptText(row.get(3)?),
            Val::Text(row.get(4)?),
            Val::Int(row.get(5)?),
        ],
        "snapshots" => vec![
            Val::Int(row.get(0)?),
            Val::Text(row.get(1)?),
            Val::Text(row.get(2)?),
        ],
        "audit_log" => vec![
            Val::Int(row.get(0)?),
            Val::Text(row.get(1)?),
            Val::Text(row.get(2)?),
            Val::Text(row.get(3)?),
            Val::OptText(row.get(4)?),
            Val::Int(row.get(5)?),
        ],
        "kv" => vec![
            Val::Text(row.get(0)?),
            Val::Text(row.get(1)?),
            Val::Int(row.get(2)?),
        ],
        _ => vec![],
    };
    Ok(vals)
}

fn table_exists(conn: &Connection, table: &str) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [table],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// Перевірка кількості рядків у Postgres (для звіту).
#[allow(dead_code)]
async fn count_rows(pool: &PgPool, table: &str) -> Result<i64> {
    let row = sqlx::query(&format!("SELECT COUNT(*) FROM {table}"))
        .fetch_one(pool)
        .await?;
    Ok(row.get(0))
}

/// Приховати пароль у URL бази даних — щоб він не потрапив у логи.
fn redact(url: &str) -> String {
    match (url.find("://"), url.find('@')) {
        (Some(scheme_end), Some(at)) if at > scheme_end => {
            let (creds, rest) = url.split_at(at);
            let user = match creds.split_once(':') {
                Some((u, _)) => u,
                None => creds,
            };
            format!("{}://{user}:***{rest}", &url[..scheme_end])
        }
        _ => url.to_string(),
    }
}
