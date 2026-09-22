//! Часто вживані адреси-константи.
//!
//! Нульова адреса зустрічалась у коді ~40 разів як сентинел (`is_junk`, фільтри
//! відправника мінту). Кожне входження — окремий шанс помилитись на один символ,
//! і така помилка тиха: фільтр просто перестає спрацьовувати.
//!
//! Значення живуть у `constants` (ключі `ADDR_*`), сюди приходять зі знімка.

use crate::snapshot;

fn c(key: &str) -> &'static str {
    snapshot().text(key).unwrap_or_else(|e| panic!("{e}"))
}

/// `0x0000…0000` — нульова адреса (мінт/бернь, порожній відправник).
pub fn zero() -> &'static str {
    c("ADDR_ZERO")
}

/// `0x0000…dead` — класична адреса спалення.
pub fn dead() -> &'static str {
    c("ADDR_DEAD")
}

pub fn weth() -> &'static str {
    c("ADDR_WETH")
}

pub fn usdt() -> &'static str {
    c("ADDR_USDT")
}

pub fn usdc() -> &'static str {
    c("ADDR_USDC")
}

/// Чи є адреса нульовою або «майже нульовою» (довгий префікс нулів).
///
/// Перевірка на префікс була розсипана по 12 файлах у вигляді
/// `a.starts_with("0x000000000000000000000000000000000000")` — сам префікс теж
/// був захардкоджений, і полічити в ньому нулі на око неможливо.
pub fn is_null_like(a: &str) -> bool {
    let a = a.trim();
    if a.eq_ignore_ascii_case(zero()) {
        return true;
    }
    // 36 нулів після 0x — те саме, що стояло в коді, але порахований машиною
    let prefix_zeros = zero().len() - 2 - 4; // весь хвіст, крім 4 останніх символів
    a.len() == 42
        && a.starts_with("0x")
        && a[2..2 + prefix_zeros].bytes().all(|b| b == b'0')
}

/// Точний відповідник `is_junk`, що був у 11 файлах nextaction:
/// нульова адреса АБО довгий префікс нулів АБО адреса, що містить "dead".
///
/// `is_empty()` СЮДИ НЕ ВХОДИТЬ навмисно. У коді було два варіанти — з ним і без,
/// і вони розходились на порожньому рядку. Зводити їх до одного тут означало б
/// тихо змінити поведінку половини файлів; хто мав перевірку, лишає її на місці.
pub fn is_junk_addr(a: &str) -> bool {
    is_null_like(a) || a.contains("dead")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready() -> bool {
        std::env::var("WR_DB").is_ok() && crate::db_path().exists()
    }

    #[test]
    fn null_like_matches_what_the_code_used_to_check() {
        if !ready() {
            return;
        }
        assert!(is_null_like("0x0000000000000000000000000000000000000000"));
        assert!(is_null_like("0x0000000000000000000000000000000000000001"));
        assert!(is_null_like("0x000000000000000000000000000000000000dead"));
        // звичайна адреса не має вважатись нульовою
        assert!(!is_null_like("0xf977814e90da44bfa03b6295a0616a897441acec"));
        assert!(!is_null_like(""));
    }

    #[test]
    fn junk_addr_keeps_the_dead_check() {
        if !ready() {
            return;
        }
        assert!(is_junk_addr("0x000000000000000000000000000000000000dead"));
        assert!(is_junk_addr("0xdead000000000000000042069420694206942069"));
        assert!(!is_junk_addr("0xf977814e90da44bfa03b6295a0616a897441acec"));
        // порожній рядок НЕ junk: перевірку на нього лишено викликачам
        assert!(!is_junk_addr(""));
    }
}
