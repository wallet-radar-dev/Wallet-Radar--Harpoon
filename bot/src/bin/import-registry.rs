//! Міграція реєстру старого `botd` у стан нового бота.
//!
//! Старий бот тримав список підключених у файлі на сервері
//! (`dataset/telegram-users.json`). Новий бот має власний стан (SQLite), тож
//! без міграції всі троє підписників мали б знову написати `/start` — і до
//! того моменту не отримували б жодного алерту.
//!
//! Використання:
//!   `import-registry <legacy.json> <bot.db>`
//!
//! Міграція **не** виконує жодних дій у Telegram — лише переносить запис про
//! підключення. Telegram-ідентифікатор (`user_id`) у старому реєстрі
//! зберігався, тому в новому стані він порожній: цього достатньо, щоб
//! `/delete` (GDPR) працював за `chat_id`.

use std::path::Path;
use storage::{Language, NewUser, Role, SqliteStore, Store, Topic};
use telegram_gateway::now_ms;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let legacy = args
        .next()
        .ok_or("вкажи шлях до старого реєстру: import-registry <legacy.json> <bot.db>")?;
    let db = args
        .next()
        .ok_or("вкажи шлях до БД нового бота: import-registry <legacy.json> <bot.db>")?;

    let raw = std::fs::read_to_string(Path::new(&legacy))?;
    let v: serde_json::Value = serde_json::from_str(&raw)?;
    let users = v["users"].as_object().cloned().unwrap_or_default();

    let store = SqliteStore::open(&db)?;
    let now = now_ms();
    let mut migrated = 0usize;

    for (chat_key, entry) in &users {
        let chat_id: i64 = match entry["chat"]
            .as_str()
            .and_then(|s| s.parse().ok())
            .or_else(|| chat_key.parse().ok())
        {
            Some(v) => v,
            None => {
                eprintln!("пропускаю запис без chat_id: {chat_key}");
                continue;
            }
        };
        let username = entry["username"].as_str().map(|s| s.to_string());
        let role = match entry["role"].as_str().unwrap_or("viewer") {
            "admin" => Role::Admin,
            _ => Role::Viewer,
        };

        store
            .upsert_user(NewUser {
                chat_id,
                user_id: None,
                username: username.clone(),
                language: Language::Uk,
                timezone: "Europe/Kyiv".into(),
                role,
            })
            .await?;

        // Підписки: переносимо людину на всі теми, щоб перехід не «вимкнув»
        // їй усе, що вона мала в старому боці.
        for topic in [
            Topic::Whale,
            Topic::Group,
            Topic::CrossAsset,
            Topic::Liquidation,
            Topic::Digest,
        ] {
            store
                .set_subscription(&storage::Subscription {
                    chat_id,
                    topic,
                    min_severity: "medium".into(),
                })
                .await?;
        }

        migrated += 1;
        println!(
            "→ {} (chat_id={chat_id}, роль={})",
            username.clone().unwrap_or_else(|| "без імені".into()),
            role.as_str()
        );
        let _ = now;
    }

    println!("перенесено записів: {migrated}");
    Ok(())
}
