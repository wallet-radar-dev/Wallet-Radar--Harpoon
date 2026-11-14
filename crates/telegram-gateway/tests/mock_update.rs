//! Тест: оновлення, яке підкладає мок Telegram, має розбиратися.
//!
//! Мок (`bot/src/bin/mock-telegram.rs`) віддає в `getUpdates` готовий JSON.
//! Якщо teloxide не розбирає такий кадр — бот мовчки не отримує жодної
//! команди, і перевірка «відповів на /health» у smoke-тесті падає без
//! пояснення. Тут перевіряємо сам розбір.
//!
//! Важлива деталь: teloxide вимагає в `from` поле `first_name` (без нього
//! `User` не розбирається, а весь кадр падає в `UpdateKind::Error` — і бот
//! не бачить помилки, бо це не помилка відповіді, а «невідомий вид оновлення»).

use teloxide::types::Update;

const MOCK_UPDATE: &str = r#"{"update_id":9001,"message":{"message_id":9001,"date":1700000000,"chat":{"id":424243,"type":"private"},"from":{"id":424243,"is_bot":false,"first_name":"Smoke","username":"vitalii_vedana"},"text":"/health"}}"#;

#[test]
fn mock_update_parses() {
    let upd: Update = serde_json::from_str(MOCK_UPDATE).expect("мок-оновлення має розбиратися");
    assert_eq!(upd.id.0, 9001);
    match &upd.kind {
        teloxide::types::UpdateKind::Message(m) => {
            assert_eq!(m.text().unwrap_or(""), "/health");
            assert_eq!(m.chat.id.0, 424243);
            assert_eq!(
                m.from.as_ref().and_then(|f| f.username.clone()).as_deref(),
                Some("vitalii_vedana")
            );
        }
        other => panic!("очікували Message, отримали {other:?}"),
    }
}
