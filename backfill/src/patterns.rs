//! patterns — НАКОПИЧУВАЛЬНА база патернів поведінки гаманців.
//!
//! Навіщо цей модуль існує. Кожен аналіз (net6k, rolemap, mmfind, tokalloc, cexsignal…)
//! писав свій JSON і жив окремо. Наступний бін починав з нуля: знову вчився відрізняти
//! контракт від EOA, знову ловив NOTOK, знову наступав на genesis-адреси. Знання, здобуте
//! кров'ю (і квотою Etherscan), не переживало власний бін.
//!
//! Тут воно живе. Правило просте: **база тільки доповнюється**. Патерн, записаний одного
//! разу, лишається назавжди; повторне спостереження оновлює докази й лічильник, але ніколи
//! не стирає ані сам патерн, ані те, хто його знайшов першим.
//!
//! Файл: dataset/backfill/wallet-patterns.jsonl — по рядку на адресу.
//! Формат рядка: {"a":addr,"p":[{"pat":код,"src":бін,"n":разів,"first":ts,"last":ts,"ev":{…}}]}
//!
//! Використання з біна:
//! ```ignore
//! let mut db = PatternDb::load(&bf);
//! db.add(&addr, Finding::new("MARKET-MAKER", "mmfind").ev("vol", 12.3));
//! db.save(&bf);   // читає файл ЗАНОВО і зливає — паралельні біни не затирають одне одного
//! ```
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Одне спостереження про адресу.
#[derive(Clone, Debug)]
pub struct Finding {
    pub pat: String,
    pub src: String,
    pub ev: Map<String, Value>,
}

impl Finding {
    pub fn new(pat: &str, src: &str) -> Self {
        Finding { pat: pat.to_string(), src: src.to_string(), ev: Map::new() }
    }
    /// Числовий доказ. Голий патерн без числа неможливо перевірити пізніше.
    pub fn ev(mut self, k: &str, v: impl Into<f64>) -> Self {
        self.ev.insert(k.into(), json!(v.into()));
        self
    }
    pub fn evs(mut self, k: &str, v: &str) -> Self {
        self.ev.insert(k.into(), json!(v));
        self
    }
    /// Доказ, що стосується конкретного токена (чи іншого підключа).
    ///
    /// Без цього мультиактивний кит виглядав як власник одного токена: `add()` зливає
    /// докази за ключем, тож восьме спостереження HOLDER затирало сім попередніх і
    /// лишало випадковий останній символ. Тепер ключі не стикаються: `usd:LINK`, `usd:PEPE`.
    pub fn evk(self, k: &str, sub: &str, v: impl Into<f64>) -> Self {
        self.ev(&format!("{k}:{sub}"), v)
    }
}

#[derive(Clone, Debug)]
pub struct Rec {
    pub pat: String,
    pub src: String,
    pub n: u64,
    pub first: u64,
    pub last: u64,
    pub ev: Map<String, Value>,
}

#[derive(Default)]
pub struct PatternDb {
    pub map: HashMap<String, Vec<Rec>>,
    added: u64,
    updated: u64,
}

impl PatternDb {
    pub fn path(bf: &str) -> String {
        format!("{bf}/wallet-patterns.jsonl")
    }

    pub fn load(bf: &str) -> Self {
        let mut db = PatternDb::default();
        let Ok(f) = File::open(Self::path(bf)) else { return db };
        for line in BufReader::with_capacity(1 << 20, f).lines().map_while(Result::ok) {
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            let Some(a) = v["a"].as_str() else { continue };
            let mut recs = Vec::new();
            for p in v["p"].as_array().map(|x| x.as_slice()).unwrap_or(&[]) {
                recs.push(Rec {
                    pat: p["pat"].as_str().unwrap_or("").to_string(),
                    src: p["src"].as_str().unwrap_or("").to_string(),
                    n: p["n"].as_u64().unwrap_or(1),
                    first: p["first"].as_u64().unwrap_or(0),
                    last: p["last"].as_u64().unwrap_or(0),
                    ev: p["ev"].as_object().cloned().unwrap_or_default(),
                });
            }
            db.map.insert(a.to_lowercase(), recs);
        }
        db
    }

    /// Додати спостереження. Той самий (pat, src) не дублюється — оновлюються докази,
    /// але `first` і автор першої знахідки лишаються недоторканими.
    pub fn add(&mut self, addr: &str, f: Finding) {
        let a = addr.to_lowercase();
        let t = now();
        let recs = self.map.entry(a).or_default();
        if let Some(r) = recs.iter_mut().find(|r| r.pat == f.pat && r.src == f.src) {
            r.n += 1;
            r.last = t;
            for (k, v) in f.ev {
                r.ev.insert(k, v);
            }
            self.updated += 1;
        } else {
            recs.push(Rec { pat: f.pat, src: f.src, n: 1, first: t, last: t, ev: f.ev });
            self.added += 1;
        }
    }

    pub fn has(&self, addr: &str, pat: &str) -> bool {
        self.map.get(&addr.to_lowercase()).map(|r| r.iter().any(|x| x.pat == pat)).unwrap_or(false)
    }

    pub fn pats(&self, addr: &str) -> Vec<&str> {
        self.map
            .get(&addr.to_lowercase())
            .map(|r| r.iter().map(|x| x.pat.as_str()).collect())
            .unwrap_or_default()
    }

    /// Скільки різних бінів незалежно назвали цей патерн. Два незалежні джерела —
    /// зовсім інша вага, ніж одне; саме тут ловиться самопідтвердження.
    pub fn support(&self, addr: &str, pat: &str) -> usize {
        let Some(r) = self.map.get(&addr.to_lowercase()) else { return 0 };
        let mut s: Vec<&str> = r.iter().filter(|x| x.pat == pat).map(|x| x.src.as_str()).collect();
        s.sort_unstable();
        s.dedup();
        s.len()
    }

    /// Записати. Перед записом файл перечитується і зливається з пам'яттю — інакше
    /// бін, що працював довше, затер би результати сусіднього. Біни ходять паралельно.
    pub fn save(&mut self, bf: &str) {
        let disk = PatternDb::load(bf);
        for (a, recs) in disk.map {
            let mine = self.map.entry(a).or_default();
            for d in recs {
                match mine.iter_mut().find(|m| m.pat == d.pat && m.src == d.src) {
                    Some(m) => {
                        // на диску може бути новіше — беремо максимум, не перезаписуємо наосліп
                        m.n = m.n.max(d.n);
                        m.first = if m.first == 0 { d.first } else { m.first.min(d.first) };
                        m.last = m.last.max(d.last);
                        for (k, v) in d.ev {
                            m.ev.entry(k).or_insert(v);
                        }
                    }
                    None => mine.push(d),
                }
            }
        }
        // ЗАПИС, ЯКИЙ НЕ ЗНИЩУЄ ДАНІ ПРИ ЗБОЇ.
        //
        // 19.08.2026 повний диск стер цю базу дощенту: помилки `writeln!` ішли в
        // `let _`, порожній tmp спокійно перейменовувався поверх робочого файлу, і
        // 27 тис. адрес зникли без жодного повідомлення. Це той самий клас тихої
        // втрати, проти якого написано половину цього крейта, — тільки в шарі
        // збереження. Тепер: кожен запис перевіряється, буфер скидається явно,
        // дані лягають на диск через fsync, і лише після цього робиться rename.
        // Будь-яка невдача лишає старий файл недоторканим.
        let tmp = format!("{}.tmp", Self::path(bf));
        let rows = {
            let f = match File::create(&tmp) {
                Ok(f) => f,
                Err(e) => {
                    eprintln!("[patterns] НЕ ЗБЕРЕЖЕНО: не створився {tmp}: {e}. Старий файл цілий.");
                    return;
                }
            };
            let mut w = std::io::BufWriter::new(f);
            let mut keys: Vec<&String> = self.map.keys().collect();
            keys.sort();
            let mut rows = 0usize;
            for a in keys {
                let recs = &self.map[a];
                if recs.is_empty() {
                    continue;
                }
                let arr: Vec<Value> = recs
                    .iter()
                    .map(|r| json!({"pat":r.pat,"src":r.src,"n":r.n,"first":r.first,"last":r.last,"ev":r.ev}))
                    .collect();
                if let Err(e) = writeln!(w, "{}", json!({"a": a, "p": arr})) {
                    eprintln!("[patterns] НЕ ЗБЕРЕЖЕНО: збій запису на рядку {rows}: {e}. Старий файл цілий.");
                    let _ = std::fs::remove_file(&tmp);
                    return;
                }
                rows += 1;
            }
            match w.into_inner() {
                Ok(f) => {
                    if let Err(e) = f.sync_all() {
                        eprintln!("[patterns] НЕ ЗБЕРЕЖЕНО: fsync не пройшов: {e}. Старий файл цілий.");
                        let _ = std::fs::remove_file(&tmp);
                        return;
                    }
                }
                Err(e) => {
                    eprintln!("[patterns] НЕ ЗБЕРЕЖЕНО: буфер не злився: {e}. Старий файл цілий.");
                    let _ = std::fs::remove_file(&tmp);
                    return;
                }
            }
            rows
        };
        // Остання перевірка перед підміною: порожній результат при непорожній базі
        // означає збій, а не «немає що зберігати».
        if rows == 0 && !self.map.is_empty() {
            eprintln!("[patterns] НЕ ЗБЕРЕЖЕНО: у tmp нуль рядків при {} адресах у пам'яті. Старий файл цілий.", self.map.len());
            let _ = std::fs::remove_file(&tmp);
            return;
        }
        if let Err(e) = std::fs::rename(&tmp, Self::path(bf)) {
            eprintln!("[patterns] НЕ ЗБЕРЕЖЕНО: rename не пройшов: {e}. Старий файл цілий.");
            let _ = std::fs::remove_file(&tmp);
            return;
        }
        eprintln!(
            "[patterns] адрес {} · нових записів {} · оновлено {}",
            self.map.len(),
            self.added,
            self.updated
        );
    }

    /// Прибрати застарілі ключі доказів. Потрібне після зміни схеми: поки докази писались
    /// під спільним ключем (`usd`, `sym`), у записі лишався випадковий останній токен —
    /// він не помилковий, але вводить в оману поряд із повним переліком `usd:СИМВОЛ`.
    pub fn drop_ev_keys(&mut self, keys: &[&str]) -> usize {
        let mut n = 0;
        for recs in self.map.values_mut() {
            for r in recs.iter_mut() {
                for k in keys {
                    if r.ev.remove(*k).is_some() {
                        n += 1;
                    }
                }
            }
        }
        n
    }

    /// Зведення: скільки адрес на кожен патерн.
    pub fn tally(&self) -> Vec<(String, usize)> {
        let mut c: HashMap<&str, usize> = HashMap::new();
        for recs in self.map.values() {
            let mut seen: Vec<&str> = recs.iter().map(|r| r.pat.as_str()).collect();
            seen.sort_unstable();
            seen.dedup();
            for p in seen {
                *c.entry(p).or_insert(0) += 1;
            }
        }
        let mut v: Vec<(String, usize)> = c.into_iter().map(|(k, n)| (k.to_string(), n)).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1));
        v
    }
}
