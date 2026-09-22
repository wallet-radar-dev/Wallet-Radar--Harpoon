//! ТИПІЗОВАНИЙ КОНФІГ З ВЕРСІЄЮ (план X.1).
//! Пороги рушія можна перевизначити з файлу `patterns.config.json` у теці даних —
//! без перекомпіляції. Якщо файлу нема / поля нема — беремо дефолт зі СЛОВНИКА (dict).
//! Так кожна вітрина відтворювана: config_version + які саме пороги її породили.
use crate::core::dict;

/// Ефективна конфігурація: типізовано, з версією, з джерелом кожного поля.
#[derive(Clone)]
pub struct Config {
    pub version: String,        // версія конфігу (файл або "dict:<DICT_VERSION>")
    pub source: String,         // "defaults (dict)" або шлях до файлу
    pub exchange_tx: u64,
    pub hub_pct: f64,
    pub min_cluster: usize,
    pub window_h: f64,
    pub grab_emission: f64,
    pub price_eth_usd: f64,
    pub price_matic_usd: f64,
    /// відносна невизначеність ціни для меж впевненості (X.4), напр. 0.15 = ±15%
    pub price_uncertainty: f64,
}

impl Config {
    /// Дефолти строго зі словника — ЄДИНЕ джерело правди за замовчуванням.
    pub fn defaults() -> Config {
        Config {
            version: format!("dict:{}", dict::dict_version()),
            source: "defaults (dict)".into(),
            exchange_tx: dict::exchange_tx(),
            hub_pct: dict::hub_pct(),
            min_cluster: dict::min_cluster(),
            window_h: dict::window_h_default(),
            grab_emission: dict::grab_emission(),
            price_eth_usd: dict::price_eth_usd(),
            price_matic_usd: dict::price_matic_usd(),
            price_uncertainty: 0.15,
        }
    }

    /// Вантажить `<dir>/patterns.config.json`, накладаючи наявні поля поверх дефолтів.
    /// Відсутній файл — не помилка (повертає дефолти). Кривий JSON — теж дефолти + прапор у source.
    pub fn load(dir: &str) -> Config {
        let mut c = Config::defaults();
        let path = format!("{dir}/patterns.config.json");
        let Ok(txt) = std::fs::read_to_string(&path) else { return c };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&txt) else {
            c.source = format!("{path} (КРИВИЙ JSON — вжито дефолти)");
            return c;
        };
        if let Some(x) = v.get("version").and_then(|x| x.as_str()) { c.version = x.into(); }
        if let Some(x) = v.get("exchange_tx").and_then(|x| x.as_u64()) { c.exchange_tx = x; }
        if let Some(x) = v.get("hub_pct").and_then(|x| x.as_f64()) { c.hub_pct = x; }
        if let Some(x) = v.get("min_cluster").and_then(|x| x.as_u64()) { c.min_cluster = x as usize; }
        if let Some(x) = v.get("window_h").and_then(|x| x.as_f64()) { c.window_h = x; }
        if let Some(x) = v.get("grab_emission").and_then(|x| x.as_f64()) { c.grab_emission = x; }
        if let Some(x) = v.get("price_eth_usd").and_then(|x| x.as_f64()) { c.price_eth_usd = x; }
        if let Some(x) = v.get("price_matic_usd").and_then(|x| x.as_f64()) { c.price_matic_usd = x; }
        if let Some(x) = v.get("price_uncertainty").and_then(|x| x.as_f64()) { c.price_uncertainty = x; }
        c.source = path;
        c
    }

    /// Межі впевненості для USD-оцінки (X.4): (низ, верх) за price_uncertainty.
    pub fn interval(&self, usd: f64) -> (f64, f64) {
        (usd * (1.0 - self.price_uncertainty), usd * (1.0 + self.price_uncertainty))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_file_falls_back_to_dict_defaults() {
        // теки нема -> дефолти словника, не паніка
        let c = Config::load("/nonexistent/dir/xyz");
        assert_eq!(c.exchange_tx, dict::exchange_tx());
        assert_eq!(c.grab_emission, dict::grab_emission());
        assert!(c.version.starts_with("dict:"));
    }
    #[test]
    fn interval_brackets_estimate() {
        let mut c = Config::defaults();
        c.price_uncertainty = 0.15;
        let (lo, hi) = c.interval(1_000_000.0);
        assert_eq!(lo, 850_000.0);
        assert_eq!(hi, 1_150_000.0);
        assert!(lo < 1_000_000.0 && hi > 1_000_000.0);
    }
}
