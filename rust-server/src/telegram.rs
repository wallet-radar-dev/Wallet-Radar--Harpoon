//! Port of server/telegram.js — signal delivery to Telegram.
//!
//! Dormant until TELEGRAM_BOT_TOKEN is in the .env: with no token it simply sends
//! nothing and breaks nothing. The chat_id resolves ITSELF — as soon as the user sends
//! the bot /start we pick up their chat (optionally only from TELEGRAM_USERNAME) and
//! cache it, so nobody has to hunt for a numeric id by hand.

use serde_json::Value;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

struct Cfg {
    token: String,
    username: String,
    chat: Mutex<String>,
}

fn cfg() -> &'static Cfg {
    static C: OnceLock<Cfg> = OnceLock::new();
    C.get_or_init(|| Cfg {
        token: std::env::var("TELEGRAM_BOT_TOKEN").unwrap_or_default(),
        username: std::env::var("TELEGRAM_USERNAME")
            .unwrap_or_default()
            .trim_start_matches('@')
            .to_lowercase(),
        chat: Mutex::new(std::env::var("TELEGRAM_CHAT_ID").unwrap_or_default()),
    })
}

fn http() -> &'static reqwest::Client {
    static C: OnceLock<reqwest::Client> = OnceLock::new();
    C.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .expect("reqwest client builds from static config")
    })
}

pub fn enabled() -> bool {
    !cfg().token.is_empty()
}

pub fn ready() -> bool {
    let c = cfg();
    !c.token.is_empty() && !c.chat.lock().unwrap().is_empty()
}

/// telegram.js `telegramStatus()`
pub fn status() -> Value {
    let c = cfg();
    serde_json::json!({
        "enabled": enabled(),
        "ready": ready(),
        "username": if c.username.is_empty() { Value::Null } else { Value::String(c.username.clone()) },
        // Скільки чатів отримує алерти: реєстр підключених + ручний chat_id.
        "targets": target_chats().len(),
    })
}

/// telegram.js `resolveChat` — find the chat_id from the bot's updates (whoever sent
/// /start). With TELEGRAM_USERNAME set, only that user's chat is accepted; otherwise the
/// most recent message wins.
pub fn pick_chat(updates: &Value, username: &str) -> Option<String> {
    let empty = vec![];
    let msgs: Vec<&Value> = updates["result"]
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .map(|u| if u["message"].is_object() { &u["message"] } else { &u["edited_message"] })
        .filter(|m| m.is_object())
        .rev()
        .collect();

    let by_user = msgs.iter().find(|m| {
        username.is_empty() || m["from"]["username"].as_str().unwrap_or("").to_lowercase() == username
    });
    let pick = by_user.or(msgs.first())?;
    let id = &pick["chat"]["id"];
    // The id is a number in the API; the JS stringifies it.
    match id {
        Value::Number(n) => Some(n.to_string()),
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        _ => None,
    }
}

async fn resolve_chat() -> String {
    let c = cfg();
    {
        let chat = c.chat.lock().unwrap();
        if !chat.is_empty() || c.token.is_empty() {
            return chat.clone();
        }
    }
    let url = format!("https://api.telegram.org/bot{}/getUpdates", c.token);
    let d: Value = match http().get(&url).send().await {
        Ok(r) if r.status().is_success() => r.json().await.unwrap_or(Value::Null),
        _ => return String::new(),
    };
    match pick_chat(&d, &c.username) {
        Some(id) => {
            tracing::info!("[telegram] chat_id визначено автоматично: {id}");
            *c.chat.lock().unwrap() = id.clone();
            id
        }
        None => String::new(),
    }
}

/// Реєстр підключених — `dataset/telegram-users.json`. Його пише бот (через
/// `POST /api/telegram-users`), і це ЄДИНЕ спільне місце: реєстр живе на сервері,
/// а не в копії бота, тож алерти сервера йдуть тим самим людям, що й команди бота.
fn users_path() -> PathBuf {
    let root = std::env::var("APP_ROOT").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from(".."));
    root.join("dataset").join("telegram-users.json")
}

/// Прочитати реєстр (порожній, якщо файлу немає).
pub fn load_users() -> Value {
    std::fs::read_to_string(users_path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .filter(|v: &Value| v["users"].is_object())
        .unwrap_or_else(|| serde_json::json!({ "users": {} }))
}

/// Записати реєстр атомарно: спершу .tmp, потім перейменування. Сервер читає цей
/// файл у будь-який момент, тож обрізаний на half write реєстр — це втрата доступу
/// для всіх підключених.
pub fn save_users(v: &Value) -> bool {
    let path = users_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = path.with_extension(format!("json.tmp.{}", std::process::id()));
    match serde_json::to_string_pretty(v) {
        Ok(body) => {
            if std::fs::write(&tmp, body).is_err() {
                return false;
            }
            let ok = std::fs::rename(&tmp, &path).is_ok();
            if !ok {
                let _ = std::fs::remove_file(&tmp);
            }
            ok
        }
        Err(_) => false,
    }
}

/// Кому слати: усі підключені (крім тих, хто ввімкнув тишу) плюс TELEGRAM_CHAT_ID
/// (якщо він заданий) — щоб стара ручна настройка не зникла з розсилки.
pub fn target_chats() -> Vec<String> {
    targets_from(&load_users())
}

/// Вибір цілей із реєстру — чиста функція (без файлу й оточення), щоб її можна
/// було перевірити тестом, не чіпаючи глобальний стан процесу.
fn targets_from(users: &Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if let Some(map) = users["users"].as_object() {
        for u in map.values() {
            if u["muted"].as_bool().unwrap_or(false) {
                continue;
            }
            let chat = u["chat"].as_str().unwrap_or("").trim().to_string();
            if !chat.is_empty() && !out.contains(&chat) {
                out.push(chat);
            }
        }
    }
    let c = cfg();
    let chat = c.chat.lock().unwrap().clone();
    if !chat.is_empty() && !out.contains(&chat) {
        out.push(chat);
    }
    out
}

/// telegram.js `sendTelegram` — HTML message, previews off. Every failure is silent:
/// a broken notification channel must never take down a worker.
///
/// Тепер це розсилка: одне повідомлення йде ВСІМ підключеним (хто не ввімкнув
/// тишу), а не одному чату. Повертає true, якщо хоч кому дійшло.
pub async fn send(text: &str) -> bool {
    let c = cfg();
    if c.token.is_empty() {
        return false;
    }
    let targets = target_chats();
    let targets = if targets.is_empty() {
        // Реєстру ще немає — піднімаємо старий чат, щоб алерти не зникли.
        let chat = {
            let chat = c.chat.lock().unwrap().clone();
            chat
        };
        let chat = if chat.is_empty() { resolve_chat().await } else { chat };
        if chat.is_empty() {
            return false;
        }
        vec![chat]
    } else {
        targets
    };

    let url = format!("https://api.telegram.org/bot{}/sendMessage", c.token);
    let mut any = false;
    for chat in targets {
        let body = serde_json::json!({
            "chat_id": chat,
            "text": text,
            "parse_mode": "HTML",
            "disable_web_page_preview": true,
        });
        match http().post(&url).json(&body).send().await {
            Ok(r) if r.status().is_success() => any = true,
            Ok(r) => tracing::warn!("[telegram] чат {chat}: HTTP {}", r.status().as_u16()),
            Err(e) => tracing::warn!("[telegram] чат {chat}: {e}"),
        }
    }
    any
}

/// telegram.js `startTelegram` — keep trying to resolve the chat until the user presses
/// Start; then idle at a 10-minute heartbeat.
pub fn start() {
    if !enabled() {
        tracing::info!("[telegram] токен не задано — модуль дрімає (додай TELEGRAM_BOT_TOKEN у .env)");
        return;
    }
    tracing::info!("[telegram] увімкнено. Напиши боту /start — chat_id визначиться сам.");
    tokio::spawn(async {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        loop {
            let have = ready();
            if !have {
                resolve_chat().await;
            }
            let wait = if ready() { 600 } else { 20 };
            tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn updates() -> Value {
        json!({"result": [
            {"message": {"from": {"username": "Someone"}, "chat": {"id": 111}}},
            {"message": {"from": {"username": "Vitalii_Vedana"}, "chat": {"id": 222}}},
            {"edited_message": {"from": {"username": "Later"}, "chat": {"id": 333}}},
        ]})
    }

    #[test]
    fn the_configured_user_wins_regardless_of_who_messaged_last() {
        assert_eq!(pick_chat(&updates(), "vitalii_vedana").as_deref(), Some("222"));
    }

    #[test]
    fn without_a_username_the_most_recent_message_wins() {
        // Reversed order → the edited_message (id 333) is newest.
        assert_eq!(pick_chat(&updates(), "").as_deref(), Some("333"));
    }

    #[test]
    fn a_username_nobody_matches_falls_back_to_the_newest_chat() {
        // JS: `upd.find(...) || upd[0]` — an unmatched filter still takes the newest.
        assert_eq!(pick_chat(&updates(), "nobody").as_deref(), Some("333"));
    }

    #[test]
    fn no_updates_means_no_chat() {
        assert_eq!(pick_chat(&json!({"result": []}), ""), None);
        assert_eq!(pick_chat(&json!({}), ""), None);
    }

    /// Реєстр — спільне місце: записи з нього стають адресатами алертів.
    #[test]
    fn registry_chats_become_alert_targets_and_muted_are_skipped() {
        let users = json!({"users": {
            "111": {"chat": "111", "username": "a", "role": "viewer", "muted": false},
            "222": {"chat": "222", "username": "b", "role": "viewer", "muted": true},
            "333": {"chat": "333", "username": "c", "role": "admin", "muted": false}
        }});
        let targets = targets_from(&users);
        assert!(targets.contains(&"111".to_string()), "підключений має отримувати алерти");
        assert!(!targets.contains(&"222".to_string()), "заглушений — не має");
        assert!(targets.contains(&"333".to_string()), "адмін — має");
    }
}
