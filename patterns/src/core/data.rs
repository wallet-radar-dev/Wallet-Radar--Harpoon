//! Стрімінг txfull-*.jsonl (по рядку, без завантаження в пам'ять цілком).
use std::io::{BufRead, BufReader};
use std::fs::File;

/// Один переказ із txfull. Поля відповідають форматам local_crawl/txfull.
pub struct Tx {
    pub w: String,    // краулений гаманець
    pub k: String,    // "ext" | "int" | "token"
    pub ts: u64,      // час
    pub f: String,    // from
    pub t: String,    // to
    pub v: String,    // сира сума
    pub tok: String,  // контракт токена (для k=token)
    pub sym: String,  // символ токена
}

/// Викликає f(Tx) для кожного рядка файлу. Мовчки пропускає биті рядки/відсутній файл.
pub fn stream_txfull<F: FnMut(Tx)>(path: &str, mut f: F) {
    let Ok(file) = File::open(path) else { return };
    let rd = BufReader::with_capacity(1 << 20, file);
    for line in rd.lines() {
        let Ok(line) = line else { continue };
        if line.is_empty() { continue }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
        let g = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        let ts = v.get("ts").and_then(|x| x.as_u64())
            .or_else(|| v.get("ts").and_then(|x| x.as_str()).and_then(|s| s.parse().ok()))
            .unwrap_or(0);
        f(Tx { w: g("w").to_lowercase(), k: g("k"), ts,
               f: g("f").to_lowercase(), t: g("t").to_lowercase(),
               v: g("v"), tok: g("tok").to_lowercase(), sym: g("sym") });
    }
}

/// Спам-фільтр — визначення у словнику (core::dict).
pub use crate::core::dict::is_spam_symbol;
