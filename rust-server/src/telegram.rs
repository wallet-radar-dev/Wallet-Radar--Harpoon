//! Port of server/telegram.js — signal delivery to Telegram.
//!
//! Dormant until TELEGRAM_BOT_TOKEN is in the .env: with no token it simply sends
//! nothing and breaks nothing. The chat_id resolves ITSELF — as soon as the user sends
//! the bot /start we pick up their chat (optionally only from TELEGRAM_USERNAME) and
//! cache it, so nobody has to hunt for a numeric id by hand.

use serde_json::Value;
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

/// telegram.js `sendTelegram` — HTML message, previews off. Every failure is silent:
/// a broken notification channel must never take down a worker.
pub async fn send(text: &str) -> bool {
    let c = cfg();
    if c.token.is_empty() {
        return false;
    }
    let chat = {
        let chat = c.chat.lock().unwrap().clone();
        chat
    };
    let chat = if chat.is_empty() { resolve_chat().await } else { chat };
    if chat.is_empty() {
        return false;
    }
    let url = format!("https://api.telegram.org/bot{}/sendMessage", c.token);
    let body = serde_json::json!({
        "chat_id": chat,
        "text": text,
        "parse_mode": "HTML",
        "disable_web_page_preview": true,
    });
    matches!(http().post(&url).json(&body).send().await, Ok(r) if r.status().is_success())
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
}
