//! Санітизація вихідного тексту.
//!
//! Це останній рубіж перед Telegram: будь-яке повідомлення, яке йде користувачеві,
//! проходить через [`PrivacyFilter::scrub`]. Фільтр — це запобіжник, а не основний
//! механізм захисту: основний — конструкція типів (у доменних структурах боту
//! немає поля, куди можна покласти адресу). Фільтр існує на випадок, коли в текст
//! потрапляє щось з чужого джерела: ручний `/broadcast` адміна, текст помилки,
//! відповідь зовнішнього сервісу.
//!
//! Що вирізаємо:
//! * EVM-адреси (`0x` + 40 hex) і tx-hash (`0x` + 64 hex);
//! * base58-адреси Solana (32..=44 символи);
//! * ENS-імена (`name.eth`);
//! * посилання на explorer із конкретною адресою;
//! * будь-який hex-рядок довжиною ≥ 32 символів (запобіжник).
//!
//! Що НЕ робимо: не намагаємося розпізнати «суму окремої транзакції» в довільному
//! тексті — це структурне питання, а не текстове (у [`Metrics`](../../alert_engine/struct.Metrics.html)
//! є лише агрегати).

use regex::Regex;
use std::sync::Arc;
use unicode_normalization::UnicodeNormalization;

/// Вид вирізаної сутності.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RedactionKind {
    EvmAddress,
    TxHash,
    Base58Address,
    EnsName,
    ExplorerUrl,
    LongHex,
}

impl RedactionKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            RedactionKind::EvmAddress => "evm_address",
            RedactionKind::TxHash => "tx_hash",
            RedactionKind::Base58Address => "base58_address",
            RedactionKind::EnsName => "ens_name",
            RedactionKind::ExplorerUrl => "explorer_url",
            RedactionKind::LongHex => "long_hex",
        }
    }
}

/// Опис однієї вирізаної сутності. Значення НЕ зберігається — лише вид, позиція
/// й довжина, щоб у лог не потрапило навіть довжини адреси.
#[derive(Clone, Debug)]
pub struct Redaction {
    pub kind: RedactionKind,
    pub at: usize,
    pub len: usize,
}

/// Результат санітизації.
#[derive(Clone, Debug)]
pub struct ScrubReport {
    /// Текст, готовий до відправки.
    pub text: String,
    /// Що саме було вирізано.
    pub redactions: Vec<Redaction>,
}

impl ScrubReport {
    pub fn is_clean(&self) -> bool {
        self.redactions.is_empty()
    }

    pub fn count(&self) -> usize {
        self.redactions.len()
    }
}

/// Ділянка тексту, яку треба вирізати.
#[derive(Clone, Copy, Debug)]
pub struct Span {
    pub at: usize,
    pub len: usize,
    pub kind: RedactionKind,
}

/// Одно правило: знаходить ділянки, які треба вирізати.
pub trait Rule: Send + Sync {
    /// Знайти всі збіги в нормалізованому тексті.
    fn scan(&self, text: &str) -> Vec<Span>;
}

macro_rules! regex_rule {
    ($name:ident, $kind:expr, $re:expr) => {
        pub struct $name {
            re: Regex,
        }

        impl $name {
            pub fn new() -> Result<Self, regex::Error> {
                Ok(Self {
                    re: Regex::new($re)?,
                })
            }
        }

        impl Rule for $name {
            fn scan(&self, text: &str) -> Vec<Span> {
                self.re
                    .find_iter(text)
                    .map(|m| Span {
                        at: m.start(),
                        len: m.end() - m.start(),
                        kind: $kind,
                    })
                    .collect()
            }
        }
    };
}

/// Hex-послідовності: адреса (40), tx-hash (64) і все, що довше.
///
/// Одне правило замість трьох — і це не компроміс, а точність: движок регулярок
/// Rust не підтримує lookahead, тож «0x + 40 hex, але не далі» через регулярку
/// не виразити. Зате довжину збігу видно просто: 40 — адреса, 64 — хеш, інше —
/// запобіжник. Побічний ефект — tx-hash не обрізається до перших 40 символів.
pub struct HexRunRule {
    re: Regex,
}

impl HexRunRule {
    pub fn new() -> Result<Self, regex::Error> {
        Ok(Self {
            re: Regex::new(r"(?i)0x[0-9a-f]{32,}")?,
        })
    }
}

impl Rule for HexRunRule {
    fn scan(&self, text: &str) -> Vec<Span> {
        self.re
            .find_iter(text)
            .map(|m| {
                let len = m.end() - m.start();
                let kind = match len {
                    40 => RedactionKind::EvmAddress,
                    64 => RedactionKind::TxHash,
                    _ => RedactionKind::LongHex,
                };
                Span {
                    at: m.start(),
                    len,
                    kind,
                }
            })
            .collect()
    }
}

// ENS.
regex_rule!(
    EnsNameRule,
    RedactionKind::EnsName,
    r"(?i)\b[a-z0-9][a-z0-9-]{1,62}\.eth\b"
);

/// Solana-адреси: base58, довжина 32..=44. Правило навмисно вузьке — ширше з'їдало б
/// звичайні слова.
pub struct Base58Rule {
    re: Regex,
}

impl Base58Rule {
    pub fn new() -> Result<Self, regex::Error> {
        // Base58 без 0, O, I, l. Межі — щоб не зачепити звичайний текст.
        Ok(Self {
            re: Regex::new(r"\b[1-9A-HJ-NP-Za-km-z]{32,44}\b")?,
        })
    }
}

impl Rule for Base58Rule {
    fn scan(&self, text: &str) -> Vec<Span> {
        self.re
            .find_iter(text)
            .map(|m| Span {
                at: m.start(),
                len: m.end() - m.start(),
                kind: RedactionKind::Base58Address,
            })
            .collect()
    }
}

/// Домени explorer'ів, у посиланнях на які ми не пускаємо нічого.
pub const DEFAULT_BLOCKED_DOMAINS: &[&str] = &[
    "etherscan.io",
    "bscscan.com",
    "arbiscan.io",
    "polygonscan.com",
    "snowtrace.io",
    "ftmscan.com",
    "lineascan.build",
    "solscan.io",
    "blockscout.com",
    "dexscreener.com",
    "tronscan.org",
    "oklink.com",
    "bitquery.io",
    "whaleslab.com",
    "arkham.intelligence",
    "debank.com",
    "etherscan.io",
];

/// Правило: URL, у домені якого є заблокований explorer, або в шляху — «0x…».
pub struct ExplorerUrlRule {
    url_re: Regex,
    hex_re: Regex,
    domains: Vec<String>,
}

impl ExplorerUrlRule {
    pub fn new(domains: &[String]) -> Result<Self, regex::Error> {
        Ok(Self {
            url_re: Regex::new(r#"(?i)\bhttps?://[^\s<>()\[\]"'`]+"#)?,
            hex_re: Regex::new(r"(?i)0x[0-9a-f]{8,}")?,
            domains: domains.iter().map(|d| d.to_lowercase()).collect(),
        })
    }
}

impl Rule for ExplorerUrlRule {
    fn scan(&self, text: &str) -> Vec<Span> {
        let lower = text.to_lowercase();
        self.url_re
            .find_iter(text)
            .filter(|m| {
                let u = &lower[m.start()..m.end()];
                let blocked_domain = self.domains.iter().any(|d| u.contains(d.as_str()));
                // Адреса в шляху — це посилання на конкретну транзакцію/гаманець.
                let hex_in_path = self.hex_re.is_match(u);
                blocked_domain || hex_in_path
            })
            .map(|m| Span {
                at: m.start(),
                len: m.end() - m.start(),
                kind: RedactionKind::ExplorerUrl,
            })
            .collect()
    }
}

/// Санітизатор вихідного тексту.
#[derive(Clone)]
pub struct PrivacyFilter {
    mask: String,
    rules: Arc<Vec<Box<dyn Rule>>>,
}

impl std::fmt::Debug for PrivacyFilter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PrivacyFilter")
            .field("mask", &self.mask)
            .field("rules", &self.rules.len())
            .finish()
    }
}

impl Default for PrivacyFilter {
    fn default() -> Self {
        Self::new("приховано")
    }
}

impl PrivacyFilter {
    /// Стандартний набір правил.
    pub fn new(mask: impl Into<String>) -> Self {
        Self::with_extra_domains(mask, &[])
    }

    /// Набір правил із перемикачами: що саме ми вирізаємо.
    pub fn with_options(
        mask: impl Into<String>,
        ens: bool,
        base58: bool,
        explorer_urls: bool,
        extra_domains: &[String],
    ) -> Self {
        let mut domains: Vec<String> = Vec::new();
        if explorer_urls {
            domains.extend(DEFAULT_BLOCKED_DOMAINS.iter().map(|d| d.to_string()));
            domains.extend(extra_domains.iter().cloned());
        }
        let mut rules: Vec<Box<dyn Rule>> = vec![Box::new(
            HexRunRule::new().expect("регулярка HexRunRule валідна"),
        )];
        if ens {
            rules.push(Box::new(
                EnsNameRule::new().expect("регулярка EnsNameRule валідна"),
            ));
        }
        if base58 {
            rules.push(Box::new(
                Base58Rule::new().expect("регулярка Base58Rule валідна"),
            ));
        }
        if explorer_urls {
            rules.push(Box::new(
                ExplorerUrlRule::new(&domains).expect("регулярка ExplorerUrlRule валідна"),
            ));
        }
        Self {
            mask: mask.into(),
            rules: Arc::new(rules),
        }
    }

    /// Стандартний набір плюс домени зі конфіга.
    pub fn with_extra_domains(mask: impl Into<String>, extra_domains: &[String]) -> Self {
        let mut domains: Vec<String> = DEFAULT_BLOCKED_DOMAINS
            .iter()
            .map(|d| d.to_string())
            .collect();
        domains.extend(extra_domains.iter().cloned());
        let rules: Vec<Box<dyn Rule>> = vec![
            Box::new(HexRunRule::new().expect("регулярка HexRunRule валідна")),
            Box::new(EnsNameRule::new().expect("регулярка EnsNameRule валідна")),
            Box::new(Base58Rule::new().expect("регулярка Base58Rule валідна")),
            Box::new(ExplorerUrlRule::new(&domains).expect("регулярка ExplorerUrlRule валідна")),
        ];
        Self {
            mask: mask.into(),
            rules: Arc::new(rules),
        }
    }

    /// Набір правил під тест ( або під нестандартну політику).
    pub fn with_rules(mask: impl Into<String>, rules: Vec<Box<dyn Rule>>) -> Self {
        Self {
            mask: mask.into(),
            rules: Arc::new(rules),
        }
    }

    pub fn mask(&self) -> &str {
        &self.mask
    }

    /// Нормалізація перед пошуком: NFKC (щоб повноширинні «０х» не ховалися від
    /// регулярки) і зрізання zero-width символів (ними ховають адреси).
    fn normalize(text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        for ch in text.nfkc() {
            if matches!(ch as u32, 0x200B..=0x200F | 0xFEFF | 0x00AD) {
                continue;
            }
            out.push(ch);
        }
        out
    }

    /// Санітизувати текст.
    pub fn scrub(&self, text: &str) -> ScrubReport {
        let normalized = Self::normalize(text);
        let mut spans: Vec<(usize, usize, RedactionKind)> = Vec::new();

        for rule in self.rules.iter() {
            for span in rule.scan(&normalized) {
                spans.push((span.at, span.len, span.kind));
            }
        }

        // Перекриті ділянки зливаємо: щоб не було «приховано» + залишку хвоста.
        spans.sort_by_key(|(at, len, _)| (*at, std::cmp::Reverse(*len)));
        let mut merged: Vec<(usize, usize, RedactionKind)> = Vec::with_capacity(spans.len());
        for (at, len, kind) in spans {
            match merged.last_mut() {
                Some(last) if at < last.0 + last.1 => {
                    let end = std::cmp::max(last.0 + last.1, at + len);
                    last.1 = end - last.0;
                }
                _ => merged.push((at, len, kind)),
            }
        }

        if merged.is_empty() {
            return ScrubReport {
                text: normalized,
                redactions: Vec::new(),
            };
        }

        let mut out = String::with_capacity(normalized.len());
        let mut cursor = 0usize;
        let mut redactions = Vec::with_capacity(merged.len());
        for (at, len, kind) in merged {
            if at < cursor {
                continue;
            }
            out.push_str(&normalized[cursor..at]);
            out.push_str(&self.mask);
            redactions.push(Redaction { kind, at, len });
            cursor = at + len;
        }
        out.push_str(&normalized[cursor..]);

        ScrubReport {
            text: out,
            redactions,
        }
    }
}

/// Трейт для залежностей, які мають фільтрувати свій вихід.
pub trait Sanitizer {
    fn scrub(&self, text: &str) -> ScrubReport;
}

impl Sanitizer for PrivacyFilter {
    fn scrub(&self, text: &str) -> ScrubReport {
        PrivacyFilter::scrub(self, text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f() -> PrivacyFilter {
        PrivacyFilter::new("приховано")
    }

    #[test]
    fn evm_address_is_masked() {
        let r = f().scrub("кит 0x611f7bf868a6212f871e89f7e44684045ddfb09d накопичує");
        assert!(!r.text.contains("0x611f"), "текст: {}", r.text);
        assert!(r.text.contains("приховано"));
        assert_eq!(r.redactions.len(), 1);
    }

    #[test]
    fn tx_hash_is_masked_and_not_truncated_to_an_address() {
        let h = "0xb25dcebf917db28dd7702336db997e2dd6c149371c628743be68c8dbf3a7b6c2";
        let r = f().scrub(&format!("tx {h}"));
        assert!(!r.text.contains("0xb25d"), "текст: {}", r.text);
        assert_eq!(r.count(), 1, "має бути одна заміна, а не дві");
    }

    #[test]
    fn solana_base58_address_is_masked() {
        let addr = "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM";
        let r = f().scrub(&format!("Solana {addr} кит"));
        assert!(!r
            .text
            .contains("9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM"));
    }

    #[test]
    fn explorer_url_with_address_is_masked() {
        let r = f().scrub("https://etherscan.io/tx/0xb25dcebf917db28dd7702336db997e2dd6c149371c628743be68c8dbf3a7b6c2");
        assert!(!r.text.contains("etherscan.io"), "текст: {}", r.text);
    }

    #[test]
    fn plain_domain_without_address_is_not_masked() {
        let r = f().scrub("див. документацію на example.com");
        assert!(r.is_clean());
        assert_eq!(r.text, "див. документацію на example.com");
    }

    #[test]
    fn ens_name_is_masked() {
        let r = f().scrub("whale.eth накопичує");
        assert!(!r.text.contains("whale.eth"));
    }

    #[test]
    fn zero_width_characters_do_not_hide_an_address() {
        let hidden = "0x611f7bf868a6212f871e89f7e44684045ddfb09d";
        let with_zw = "0x611f7bf8\u{200b}68a6212f871e89f7e44684045ddfb09d".to_string();
        let r = f().scrub(&with_zw);
        assert!(!r.text.contains("611f7bf8"), "текст: {}", r.text);
        assert!(hidden.starts_with("0x611f7bf8"));
    }

    #[test]
    fn fullwidth_characters_do_not_hide_an_address() {
        // Повноширинні ０ і х — NFKC має привести їх до ASCII.
        let full = "\u{ff10}\u{ff58}611f7bf868a6212f871e89f7e44684045ddfb09d";
        let r = f().scrub(full);
        assert!(!r.text.contains("611f7bf8"), "текст: {}", r.text);
    }

    #[test]
    fn ordinary_text_is_untouched() {
        let s = "Группа A накопичила SOL: 3 участника, $42 800 за 15 минут";
        let r = f().scrub(s);
        assert!(r.is_clean());
        assert_eq!(r.text, s);
    }

    #[test]
    fn short_hex_sequences_are_not_masked() {
        // 0x + 8 hex — це не адреса; таке зустрічається в номерах блоків.
        let r = f().scrub("блок 0xdeadbeef підтверджено");
        assert!(r.is_clean(), "текст: {}", r.text);
    }

    #[test]
    fn report_does_not_leak_the_masked_value() {
        let r = f().scrub("0x611f7bf868a6212f871e89f7e44684045ddfb09d");
        // У звіті є довжина — але не саме значення.
        assert_eq!(r.redactions[0].len, 42);
        assert!(!format!("{:?}", r.redactions).contains("611f7bf8"));
    }
}
