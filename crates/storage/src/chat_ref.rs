//! Псевдонім чату (`chat_ref`) для аудиту й логів.
//!
//! У журналі аудиту та в логах не має бути самого `chat_id` — лише
//! псевдонім, який без секрету не відновити. `chat_ref` — це HMAC-SHA256
//! від `chat_id` із секретом підписування; без секрету це просто хеш
//! (відновити перебором), із секретом — HMAC (без ключа не відновити).
//!
//! Модуль спільний для всіх бекендів сховища, щоб `chat_ref` для того самого
//! чату однаково виглядав у SQLite та Postgres.

use sha2::{Digest, Sha256};

/// HMAC-SHA256 — щоб `chat_ref` у аудиті не можна було відновити перебором.
pub(crate) struct HmacSha256 {
    inner: Sha256,
    outer_key: [u8; 64],
}

impl HmacSha256 {
    /// Створює HMAC з ключем (ключ довший за 64 байти — зводиться до SHA-256).
    pub(crate) fn new(key: &[u8]) -> HmacSha256 {
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

    /// Додає дані до повідомлення.
    pub(crate) fn update(&mut self, data: &[u8]) {
        self.inner.update(data);
    }

    /// Завершує обчислення і повертає hex-рядок.
    pub(crate) fn finish_hex(self) -> String {
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

/// Псевдонім чату: HMAC від chat_id із секретом.
pub(crate) fn chat_ref_with(secret: &[u8], chat_id: i64) -> String {
    let mut mac = HmacSha256::new(secret);
    mac.update(&chat_id.to_le_bytes());
    format!("chat_{}", &mac.finish_hex()[..16])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn чат_реф_стабільний_і_не_розкриває_chat_id() {
        let a = chat_ref_with(b"secret", 42);
        let b = chat_ref_with(b"secret", 42);
        assert_eq!(a, b);
        assert!(a.starts_with("chat_"));
        assert!(!a.contains("42"));
    }

    #[test]
    fn різні_секрети_дають_різні_псевдоніми() {
        assert_ne!(chat_ref_with(b"a", 42), chat_ref_with(b"b", 42));
    }
}
