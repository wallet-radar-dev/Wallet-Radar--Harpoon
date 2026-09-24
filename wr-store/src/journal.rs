//! journal — незмінний журнал сигналів і їхніх наслідків.
//!
//! Навіщо. Сигнали в нас народжуються в різних бінах (selltape шле сплески в стакані,
//! cascalert — роздачу, groupsig — зміну частки емісії) і вмирають у Telegram. Через
//! місяць немає жодного способу сказати, чи вони справді працювали: повідомлення
//! прокрутилось у стрічці, ціна на момент події ніде не збереглась, а пам'ять
//! підказує лише ті випадки, які вразили. Це найгірший режим для дослідження —
//! суб'єктивна вибірка з власних спогадів.
//!
//! Тому: КОЖЕН сигнал пишеться сюди один раз, у момент народження, разом із ціною
//! на той момент. Файл тільки дописується — рядок, який тут опинився, не
//! переписується ніколи. Наслідок (що сталося з ціною через N днів) лягає в ОКРЕМИЙ
//! файл і посилається на сигнал за `id`. Саме через це журнал не можна підігнати
//! заднім числом: запис про передбачення фізично старший за запис про результат.
//!
//! Два файли, обидва JSONL (рядок = один об'єкт):
//!   * `signals.jsonl`  — що ми стверджували і коли;
//!   * `outcomes.jsonl` — чим це скінчилось, окремий рядок на кожен горизонт.
//!
//! Чому не SQLite, коли поруч є `wr-store`. База тут — довідник: її рядки
//! оновлюються. Журналу ж потрібна протилежна властивість — незмінність і
//! дописування з кількох процесів одночасно. Дозапис одного короткого рядка у файл,
//! відкритий у режимі append, атомарний і не потребує блокувань; UPDATE у спільній
//! базі з п'яти бінів — потребує.
//!
//! Пошкоджені рядки. Процес може загинути посеред запису (у нас гине хост), тож
//! читач НЕ падає на битому рядку — він його пропускає, але рахує. Кількість
//! пропущених віддається назовні (`bad_lines`), і звіт зобов'язаний її показати:
//! тиха втрата рядків — рівно та помилка, через яку цей журнал і заводиться.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

/// Версія схеми запису. Зростає, коли змінюється СЕНС полів, а не коли додається
/// нове поле: старі рядки мусять читатися вічно, інакше журнал втрачає цінність.
pub const SCHEMA: u32 = 1;

/// Що ми стверджували і коли. Пишеться один раз, не редагується.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Signal {
    #[serde(default = "one")]
    pub v: u32,
    /// Ключ ідемпотентності, `джерело:актив:вид:вікно`. Повторний прогін того самого
    /// вікна (перезапуск, ручний запуск) не має плодити другий запис про ту саму подію.
    pub id: String,
    /// Час САМОЇ ПОДІЇ (початок закритого вікна), UTC, секунди. Від нього рахуються горизонти.
    pub ts: i64,
    /// Коли рядок записано. Відрізняється від `ts` на затримку проходу — і саме ця
    /// різниця показує, чи можна було діяти за сигналом у реальності.
    pub logged_at: i64,
    /// Хто згенерував: `selltape`, `cascalert`, `groupsig`…
    pub source: String,
    /// Напрям твердження: `buy` — очікуємо силу, `sell` — слабкість.
    pub kind: String,
    pub sym: String,
    /// Контракт активу й мережа. Саме за ними дозрівання бере ціну з DefiLlama —
    /// тикер для цього непридатний: однакових тикерів у нас уже ловили цілу купку,
    /// і ціну підставляло від чужого токена.
    #[serde(default)]
    pub contract: String,
    #[serde(default)]
    pub chain: String,
    /// Біржа й пара, з яких узята `price0`. Ціна ЗАВЖДИ біржова: он-чейн ціни в нас
    /// доведено брешуть (COMP ×2350), тому журнал їх не приймає.
    pub venue: String,
    pub pair: String,
    /// Ціна на момент сигналу. Нуль неприпустимий — сигнал без ціни неперевірний.
    pub price0: f64,
    /// Сила сигналу в термінах джерела: сплеск, частка емісії, обсяг. Довільний
    /// об'єкт, бо в кожного джерела своя лінійка; звіт ріже вибірку саме за цими числами.
    #[serde(default)]
    pub strength: serde_json::Value,
    /// Чи пішло повідомлення в Telegram. `None` — на момент запису ще не відомо, і це
    /// нормальний стан: джерело пише сигнал ДО спроби відправки, бо сигнал — це знахідка
    /// детектора, а не доставлене повідомлення. Брехливе `false` тут гірше за чесне «не знаю».
    #[serde(default)]
    pub sent: Option<bool>,
    #[serde(default)]
    pub note: String,
}

fn one() -> u32 { 1 }

/// Чим скінчилось. Окремий рядок на кожен горизонт кожного сигналу.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Outcome {
    #[serde(default = "one")]
    pub v: u32,
    /// `Signal::id`, до якого належить результат.
    pub id: String,
    /// Горизонт у днях: 1, 7, 30, 90.
    pub horizon_d: u32,
    pub checked_at: i64,
    /// Ціни беруться з ОДНОГО джерела на обидва кінці вікна: інакше різниця між
    /// біржами підмішується в дохідність і виглядає як сигнал.
    pub price0: Option<f64>,
    pub price1: Option<f64>,
    pub ret_pct: Option<f64>,
    /// Ринок за те саме вікно (BTC). Без нього «ціна зросла» нічого не означає:
    /// у бичачому тижні зростає майже все.
    pub bench_ret_pct: Option<f64>,
    /// `ret_pct - bench_ret_pct`. Єдине число, за яким взагалі можна судити.
    pub excess_pct: Option<f64>,
    /// `ok` або причина, чому не порахувалось.
    pub status: String,
    /// Непорожнє завжди, коли `status != "ok"`.
    #[serde(default)]
    pub reason: String,
}

impl Outcome {
    /// Результат не порахувався. Конструктор існує, щоб неможливо було створити
    /// невдачу без причини: порожнє пояснення — це тиха втрата.
    pub fn failed(id: &str, horizon_d: u32, now: i64, status: &str, reason: &str) -> Outcome {
        debug_assert!(!reason.is_empty(), "невдалий результат без причини");
        Outcome {
            v: SCHEMA,
            id: id.to_string(),
            horizon_d,
            checked_at: now,
            price0: None,
            price1: None,
            ret_pct: None,
            bench_ret_pct: None,
            excess_pct: None,
            status: status.to_string(),
            reason: if reason.is_empty() { "причину не вказано".into() } else { reason.to_string() },
        }
    }

    /// Чи справдився сигнал: для `buy` — надлишок понад ринок, для `sell` — просідання
    /// відносно ринку. None, коли рахувати нема з чого.
    pub fn hit(&self, kind: &str) -> Option<bool> {
        let e = self.excess_pct?;
        match kind {
            "buy" => Some(e > 0.0),
            "sell" => Some(e < 0.0),
            _ => None,
        }
    }

    pub fn key(&self) -> (String, u32) { (self.id.clone(), self.horizon_d) }
}

#[derive(Debug)]
pub enum JErr {
    Io(std::io::Error),
    Bad(String),
}

impl std::fmt::Display for JErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JErr::Io(e) => write!(f, "журнал: {e}"),
            JErr::Bad(s) => write!(f, "журнал: {s}"),
        }
    }
}
impl std::error::Error for JErr {}
impl From<std::io::Error> for JErr {
    fn from(e: std::io::Error) -> Self { JErr::Io(e) }
}

/// Тека журналу: `SIGNAL_JOURNAL_DIR`, інакше `DATASET_DIR`, інакше поточна.
///
/// Свідомо той самий корінь, що й у решти даних: журнал — такий самий набір даних,
/// а не службовий лог. Він має їхати разом із датасетом у бекап і в git.
pub fn journal_dir() -> PathBuf {
    if let Ok(p) = std::env::var("SIGNAL_JOURNAL_DIR") {
        if !p.trim().is_empty() { return PathBuf::from(p) }
    }
    if let Ok(p) = std::env::var("DATASET_DIR") {
        if !p.trim().is_empty() { return PathBuf::from(p) }
    }
    PathBuf::from(".")
}

/// Те саме, але з відомим коренем проєкту (бін отримує його першим аргументом —
/// стала домовленість усіх бінів репозиторію).
pub fn journal_dir_for(root: &str) -> PathBuf {
    if std::env::var("SIGNAL_JOURNAL_DIR").map(|v| !v.trim().is_empty()).unwrap_or(false)
        || std::env::var("DATASET_DIR").map(|v| !v.trim().is_empty()).unwrap_or(false) {
        return journal_dir()
    }
    Path::new(root).join("dataset")
}

/// Відкритий журнал: знає, які `id` уже записані, тож повтор не створює дубля.
pub struct Journal {
    dir: PathBuf,
    seen_signals: HashSet<String>,
    seen_outcomes: HashSet<(String, u32)>,
    bad_lines: usize,
}

impl Journal {
    pub fn open() -> Result<Journal, JErr> { Journal::open_at(journal_dir()) }

    pub fn open_at<P: AsRef<Path>>(dir: P) -> Result<Journal, JErr> {
        let dir = dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&dir)?;
        let mut j = Journal { dir, seen_signals: HashSet::new(), seen_outcomes: HashSet::new(), bad_lines: 0 };
        for s in j.signals()? { j.seen_signals.insert(s.id); }
        for o in j.outcomes()? { j.seen_outcomes.insert(o.key()); }
        Ok(j)
    }

    pub fn signals_path(&self) -> PathBuf { self.dir.join("signals.jsonl") }
    pub fn outcomes_path(&self) -> PathBuf { self.dir.join("outcomes.jsonl") }

    /// Скільки рядків не розібралось за час життя цього об'єкта. Звіт мусить це показати.
    pub fn bad_lines(&self) -> usize { self.bad_lines }

    /// Записати сигнал. `false` — такий `id` уже є, нічого не змінилось.
    ///
    /// Помилка запису НЕ ковтається: якщо журнал не пишеться, краще побачити це одразу,
    /// ніж через три місяці виявити, що перевіряти нема чого.
    pub fn append(&mut self, s: &Signal) -> Result<bool, JErr> {
        if s.id.is_empty() { return Err(JErr::Bad("сигнал без id — нічим дедуплікувати".into())) }
        if !(s.price0 > 0.0) {
            return Err(JErr::Bad(format!("{}: ціна на момент сигналу {} — такий запис неперевірний", s.id, s.price0)))
        }
        if self.seen_signals.contains(&s.id) { return Ok(false) }
        append_line(&self.signals_path(), &serde_json::to_string(s).map_err(|e| JErr::Bad(e.to_string()))?)?;
        self.seen_signals.insert(s.id.clone());
        Ok(true)
    }

    /// Записати наслідок. `false` — для цього сигналу й горизонту результат уже є.
    pub fn append_outcome(&mut self, o: &Outcome) -> Result<bool, JErr> {
        if self.seen_outcomes.contains(&o.key()) { return Ok(false) }
        append_line(&self.outcomes_path(), &serde_json::to_string(o).map_err(|e| JErr::Bad(e.to_string()))?)?;
        self.seen_outcomes.insert(o.key());
        Ok(true)
    }

    /// Пакетний дозапис сигналів: один запис на диск і одна синхронізація на всю пачку.
    ///
    /// Навіщо окремий шлях. `append` синхронізує кожен рядок — це правильно для живого
    /// монітора, де сигналів одиниці, і руйнівно для реплею, де їх десятки тисяч:
    /// сорок тисяч синхронізацій — це хвилини чекання на диск. Гарантія лишається та
    /// сама, просто на рівні пачки: або на диску вся пачка, або те, що встигло, —
    /// і жоден уже записаний рядок від цього не псується.
    pub fn append_many(&mut self, items: &[Signal]) -> Result<usize, JErr> {
        let mut buf = String::new();
        let mut fresh: Vec<String> = Vec::new();
        for s in items {
            if s.id.is_empty() || !(s.price0 > 0.0) {
                return Err(JErr::Bad(format!("{}: запис без id або без ціни — неперевірний", s.id)))
            }
            if self.seen_signals.contains(&s.id) || fresh.contains(&s.id) { continue }
            buf.push_str(&serde_json::to_string(s).map_err(|e| JErr::Bad(e.to_string()))?);
            buf.push('\n');
            fresh.push(s.id.clone());
        }
        if fresh.is_empty() { return Ok(0) }
        append_bulk(&self.signals_path(), &buf)?;
        for id in fresh.iter() { self.seen_signals.insert(id.clone()); }
        Ok(fresh.len())
    }

    /// Те саме для результатів.
    pub fn append_outcomes_many(&mut self, items: &[Outcome]) -> Result<usize, JErr> {
        let mut buf = String::new();
        let mut fresh: Vec<(String, u32)> = Vec::new();
        for o in items {
            let k = o.key();
            if self.seen_outcomes.contains(&k) || fresh.contains(&k) { continue }
            buf.push_str(&serde_json::to_string(o).map_err(|e| JErr::Bad(e.to_string()))?);
            buf.push('\n');
            fresh.push(k);
        }
        if fresh.is_empty() { return Ok(0) }
        append_bulk(&self.outcomes_path(), &buf)?;
        for k in fresh.iter() { self.seen_outcomes.insert(k.clone()); }
        Ok(fresh.len())
    }

    pub fn has_signal(&self, id: &str) -> bool { self.seen_signals.contains(id) }

    pub fn has_outcome(&self, id: &str, horizon_d: u32) -> bool {
        self.seen_outcomes.contains(&(id.to_string(), horizon_d))
    }

    pub fn signals(&mut self) -> Result<Vec<Signal>, JErr> {
        let (v, bad) = read_jsonl::<Signal>(&self.signals_path())?;
        self.bad_lines += bad;
        Ok(v)
    }

    pub fn outcomes(&mut self) -> Result<Vec<Outcome>, JErr> {
        let (v, bad) = read_jsonl::<Outcome>(&self.outcomes_path())?;
        self.bad_lines += bad;
        Ok(v)
    }
}

/// Дозапис одного рядка з примусовим скиданням на диск.
///
/// `sync_all` тут не зайва обережність: хост цієї системи регулярно гине від зникнення
/// живлення (177 аварійних вимкнень зі 259). Без синхронізації останні сигнали жили б
/// у кеші ОС і зникали саме тоді, коли найцікавіше — у момент збою.
fn append_line(path: &Path, line: &str) -> Result<(), JErr> {
    if line.contains('\n') { return Err(JErr::Bad("перенос рядка всередині запису".into())) }
    let mut fh = OpenOptions::new().create(true).append(true).open(path)?;
    // Один виклик write_all на «рядок + \n»: дозапис одним шматком не перемішується
    // з дозаписом іншого процесу, тому кілька бінів можуть писати в журнал одночасно.
    let mut buf = String::with_capacity(line.len() + 1);
    buf.push_str(line);
    buf.push('\n');
    fh.write_all(buf.as_bytes())?;
    fh.flush()?;
    fh.sync_all()?;
    Ok(())
}

/// Дозапис пачки готових рядків: одна синхронізація на все.
fn append_bulk(path: &Path, body: &str) -> Result<(), JErr> {
    if body.is_empty() { return Ok(()) }
    let mut fh = OpenOptions::new().create(true).append(true).open(path)?;
    fh.write_all(body.as_bytes())?;
    fh.flush()?;
    fh.sync_all()?;
    Ok(())
}

/// Читання JSONL: повертає розібрані записи й кількість битих рядків.
fn read_jsonl<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<(Vec<T>, usize), JErr> {
    if !path.exists() { return Ok((Vec::new(), 0)) }
    let fh = std::fs::File::open(path)?;
    let mut out = Vec::new();
    let mut bad = 0usize;
    for line in BufReader::new(fh).lines() {
        let line = line?;
        let t = line.trim();
        if t.is_empty() { continue }
        match serde_json::from_str::<T>(t) {
            Ok(v) => out.push(v),
            Err(e) => {
                bad += 1;
                eprintln!("[journal] пропущено рядок у {}: {e}", path.display());
            }
        }
    }
    Ok((out, bad))
}

/// Зручний конструктор для джерел сигналів.
#[allow(clippy::too_many_arguments)]
pub fn signal(
    source: &str, kind: &str, sym: &str, window_ts: i64, now: i64,
    venue: &str, pair: &str, price0: f64, strength: serde_json::Value,
) -> Signal {
    Signal {
        v: SCHEMA,
        id: format!("{source}:{sym}:{kind}:{window_ts}"),
        ts: window_ts,
        logged_at: now,
        source: source.to_string(),
        kind: kind.to_string(),
        sym: sym.to_string(),
        contract: String::new(),
        chain: String::new(),
        venue: venue.to_string(),
        pair: pair.to_string(),
        price0,
        strength,
        sent: None,
        note: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("wr-journal-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    fn sig(id_ts: i64) -> Signal {
        signal("selltape", "buy", "AAVE", id_ts, id_ts + 60, "binance", "AAVEUSDT", 190.38,
               serde_json::json!({"spike": 2.5, "emis_pct": 0.052}))
    }

    #[test]
    fn append_then_read_roundtrip() {
        let d = tmp("roundtrip");
        let mut j = Journal::open_at(&d).unwrap();
        assert!(j.append(&sig(1000)).unwrap());
        assert!(j.append(&sig(2000)).unwrap());
        let got = j.signals().unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].sym, "AAVE");
        assert_eq!(got[0].price0, 190.38);
        assert_eq!(got[0].strength["spike"], 2.5);
    }

    #[test]
    fn same_window_is_not_recorded_twice() {
        let d = tmp("dedupe");
        let mut j = Journal::open_at(&d).unwrap();
        assert!(j.append(&sig(1000)).unwrap());
        assert!(!j.append(&sig(1000)).unwrap(), "повтор того самого вікна не має плодити запис");
        // і після перевідкриття теж: дедуплікація не має жити лише в пам'яті процесу
        let mut j2 = Journal::open_at(&d).unwrap();
        assert!(!j2.append(&sig(1000)).unwrap());
        assert_eq!(j2.signals().unwrap().len(), 1);
    }

    #[test]
    fn signal_without_price_is_refused() {
        let d = tmp("noprice");
        let mut j = Journal::open_at(&d).unwrap();
        let mut s = sig(1000);
        s.price0 = 0.0;
        assert!(j.append(&s).is_err(), "сигнал без ціни неперевірний — приймати його не можна");
        assert_eq!(j.signals().unwrap().len(), 0);
    }

    #[test]
    fn broken_line_is_skipped_and_counted() {
        let d = tmp("broken");
        let mut j = Journal::open_at(&d).unwrap();
        j.append(&sig(1000)).unwrap();
        // імітація загибелі процесу посеред запису
        let mut fh = OpenOptions::new().append(true).open(j.signals_path()).unwrap();
        fh.write_all(b"{\"id\":\"selltape:AAVE:buy:20").unwrap();
        fh.write_all(b"\n").unwrap();
        drop(fh);
        let mut j2 = Journal::open_at(&d).unwrap();
        assert_eq!(j2.signals().unwrap().len(), 1, "цілий рядок має читатись попри битий сусідній");
        assert!(j2.bad_lines() > 0, "биті рядки мають рахуватись, а не зникати тихо");
    }

    #[test]
    fn outcome_is_keyed_by_horizon() {
        let d = tmp("outcome");
        let mut j = Journal::open_at(&d).unwrap();
        let s = sig(1000);
        j.append(&s).unwrap();
        let mut o = Outcome::failed(&s.id, 7, 100, "no-price", "біржа не віддала свічку");
        assert!(j.append_outcome(&o).unwrap());
        assert!(!j.append_outcome(&o).unwrap(), "той самий горизонт двічі не пишеться");
        o.horizon_d = 30;
        assert!(j.append_outcome(&o).unwrap(), "інший горизонт — окремий запис");
        assert!(j.has_outcome(&s.id, 7));
        assert!(!j.has_outcome(&s.id, 90));
    }

    #[test]
    fn batch_append_dedupes_within_and_across_calls() {
        let d = tmp("batch");
        let mut j = Journal::open_at(&d).unwrap();
        // у самій пачці той самий id двічі + один уже записаний раніше
        j.append(&sig(1000)).unwrap();
        let n = j.append_many(&[sig(1000), sig(2000), sig(2000), sig(3000)]).unwrap();
        assert_eq!(n, 2, "у файл мають лягти лише два нові записи");
        let mut j2 = Journal::open_at(&d).unwrap();
        let got = j2.signals().unwrap();
        assert_eq!(got.len(), 3);
        let mut ids: Vec<String> = got.into_iter().map(|s| s.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 3, "дублів у файлі бути не може");
    }

    #[test]
    fn batch_refuses_record_without_price() {
        let d = tmp("batchbad");
        let mut j = Journal::open_at(&d).unwrap();
        let mut bad = sig(2000);
        bad.price0 = 0.0;
        assert!(j.append_many(&[sig(1000), bad]).is_err());
        // відмова настає ДО запису: половини пачки на диску не лишається
        assert_eq!(j.signals().unwrap().len(), 0, "пачка з битим записом не пишеться частково");
    }

    #[test]
    fn hit_depends_on_direction() {
        let mut o = Outcome::failed("x", 7, 0, "ok", "-");
        o.excess_pct = Some(3.0);
        assert_eq!(o.hit("buy"), Some(true));
        assert_eq!(o.hit("sell"), Some(false));
        o.excess_pct = None;
        assert_eq!(o.hit("buy"), None, "без надлишку присуду немає");
    }
}
