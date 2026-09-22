//! РЕАЛЬНІ ЦІНИ з артефакту (Фаза 1.2 плану): читаємо `token-prices.json`
//! (його пише backfill/tokprices — DefiLlama, {eth, asOf, tokens{contract->usd}}).
//! Якщо файлу нема — ВІДКАТ на константи словника (не помилка). Так рушій сам почне
//! брати реальні ціни, щойно tokprices згенерує артефакт — без зміни коду патернів.
//! ВАЖЛИВО (memory token-prices-artifact): ончейн-ціни БРЕШУТЬ (COMP ×2350) — джерело тут, не ончейн.
use std::collections::HashMap;
use crate::core::dict;

pub struct Prices {
    pub eth_usd: f64,
    pub as_of: u64,
    pub source: String,               // "token-prices.json (defillama)" або "dict-fallback"
    by_contract: HashMap<String, f64>, // contract-lc -> usd
}

impl Prices {
    pub fn fallback() -> Prices {
        Prices { eth_usd: dict::price_eth_usd(), as_of: 0, source: "dict-fallback".into(), by_contract: HashMap::new() }
    }

    /// Вантажить token-prices.json. Шукає у `<dir>`, тоді у батьківській теці (bin tokprices
    /// пише у dataset/backfill/, а патерни працюють у підтеці local-from-laptop).
    /// Значення tokens приймаємо як число АБО обʼєкт з price/usd. Відсутній/кривий -> fallback.
    pub fn load(dir: &str) -> Prices {
        let candidates = [
            format!("{dir}/token-prices.json"),
            format!("{dir}/../token-prices.json"), // dataset/backfill/token-prices.json
        ];
        let Some(txt) = candidates.iter().find_map(|p| std::fs::read_to_string(p).ok()) else {
            return Prices::fallback();
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&txt) else { return Prices::fallback() };
        let eth = v.get("eth").and_then(|x| x.as_f64()).unwrap_or(dict::price_eth_usd());
        let as_of = v.get("asOf").and_then(|x| x.as_u64()).unwrap_or(0);
        let src = v.get("source").and_then(|x| x.as_str()).unwrap_or("defillama");
        let mut by_contract = HashMap::new();
        if let Some(tok) = v.get("tokens").and_then(|x| x.as_object()) {
            for (k, val) in tok {
                let usd = val.as_f64()
                    .or_else(|| val.get("price").and_then(|x| x.as_f64()))
                    .or_else(|| val.get("usd").and_then(|x| x.as_f64()));
                if let Some(u) = usd { if u > 0.0 { by_contract.insert(k.to_lowercase(), u); } }
            }
        }
        Prices { eth_usd: eth, as_of, source: format!("token-prices.json ({src})"), by_contract }
    }

    /// USD-ціна токена за контрактом (реальна, з артефакту). None якщо ціни нема.
    pub fn usd_of(&self, contract: &str) -> Option<f64> {
        self.by_contract.get(&contract.to_lowercase()).copied()
    }

    /// Скільки контрактів має реальну ціну (0 = працюємо на fallback-константах).
    pub fn priced_count(&self) -> usize { self.by_contract.len() }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_file_falls_back() {
        let p = Prices::load("/nonexistent/xyz");
        assert_eq!(p.eth_usd, dict::price_eth_usd());
        assert_eq!(p.source, "dict-fallback");
        assert_eq!(p.priced_count(), 0);
        assert!(p.usd_of("0xabc").is_none());
    }
    #[test]
    fn parses_number_and_object_forms() {
        // будуємо Prices вручну через both-форми в памʼяті
        let mut by = HashMap::new();
        by.insert("0xaaa".to_string(), 1.5);
        let p = Prices { eth_usd: 3200.0, as_of: 1, source: "test".into(), by_contract: by };
        assert_eq!(p.usd_of("0xAAA"), Some(1.5)); // регістронезалежно
        assert_eq!(p.priced_count(), 1);
    }
}
