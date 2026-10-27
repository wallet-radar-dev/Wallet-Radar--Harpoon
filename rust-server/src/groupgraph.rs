//! Заглушка. Справжній `groupgraph.rs` у публічну версію репо не входить (див. README),
//! а без модуля `rust-server` не компілюється взагалі. Заглушка тримає збірку живою:
//! `/api/group-graph` відповідає 501 замість того, щоб валити весь сервер.
//!
//! Справжній файл кладеться на місце цього — сигнатуру `build` міняти не треба.

use serde_json::{Value, json};

pub fn build(_root: &str, _id: &str, _limit: usize) -> Value {
    json!({
        "error": "граф групи недоступний: groupgraph.rs відсутній у цій збірці",
        "unavailable": true,
    })
}
