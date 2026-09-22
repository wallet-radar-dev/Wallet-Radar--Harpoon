//! Дзеркалення бази на кілька носіїв.
//!
//! Просте копіювання файла тут НЕ працює. База в режимі WAL тримає частину
//! свіжих транзакцій у `-wal`, тож `copy wallet-radar.db` дає або зіпсований файл,
//! або мовчки застарілий — на всі незлиті транзакції. Це рівно той клас помилки,
//! якого ми уникаємо: резервна копія, яка виглядає цілою і не є нею.
//!
//! Тому `VACUUM INTO` — SQLite сам складає узгоджений знімок на вказаний шлях.
//! Запис іде у тимчасовий файл поруч і лише потім перейменовується на місце,
//! щоб перерване дзеркалення не залишило напівфайл замість попередньої копії.

use crate::{Error, Result};
use rusqlite::Connection;
use std::path::{Path, PathBuf};

/// Куди дзеркалити: `WR_DB_MIRRORS`, шляхи через `;` (Windows) або `,`.
pub fn mirror_targets() -> Vec<PathBuf> {
    std::env::var("WR_DB_MIRRORS")
        .unwrap_or_default()
        .split([';', ','])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .collect()
}

#[derive(Debug)]
pub struct MirrorOutcome {
    pub dest: PathBuf,
    pub bytes: u64,
    pub error: Option<String>,
}

impl MirrorOutcome {
    pub fn ok(&self) -> bool {
        self.error.is_none()
    }
}

/// Узгоджений знімок бази у `dest`. Перезаписує наявний файл атомарно.
pub fn snapshot_to(c: &Connection, dest: &Path) -> Result<u64> {
    // Відносний шлях тут — завжди наслідок поламаного налаштування, а не намір.
    // Спіймано дослідом: bash з'їв зворотні слеші у `WR_DB_MIRRORS`, і
    // `C:\Users\…` перетворилось на `C:Users…`, тобто «відносно поточної теки
    // на диску C:». Копія лягла всередину репозиторію, а звіт сказав «ok».
    // Тиха копія не туди гірша за відсутність копії: на неї розраховують.
    if !dest.is_absolute() {
        return Err(Error::Other(format!(
            "ціль дзеркала не абсолютний шлях: {} — схоже, у WR_DB_MIRRORS з'їдено слеші \
             (використовуй прямі: D:/wr-live/store/...)",
            dest.display()
        )));
    }
    if let Some(dir) = dest.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir)
                .map_err(|e| Error::Other(format!("не створити {}: {e}", dir.display())))?;
        }
    }
    // VACUUM INTO відмовляється писати у наявний файл, тож завжди через тимчасовий.
    let tmp = dest.with_extension("db.tmp");
    let _ = std::fs::remove_file(&tmp);

    let tmp_sql = tmp.to_string_lossy().replace('\'', "''");
    c.execute_batch(&format!("VACUUM INTO '{tmp_sql}'"))?;

    let bytes = std::fs::metadata(&tmp)
        .map_err(|e| Error::Other(format!("знімок не створився: {e}")))?
        .len();
    if bytes == 0 {
        let _ = std::fs::remove_file(&tmp);
        return Err(Error::Other("знімок нульового розміру — відмовляюсь ставити його на місце".into()));
    }

    // На Windows rename перезаписує наявний файл, тож попередня копія живе
    // до останньої миті й ніколи не буває напівзаписаною.
    std::fs::rename(&tmp, dest)
        .map_err(|e| Error::Other(format!("не поставити знімок на {}: {e}", dest.display())))?;
    Ok(bytes)
}

/// Дзеркалити на всі цілі. Недоступна ціль (відключений диск) — НЕ падіння:
/// решта копій мусить зробитись. Але результат по кожній повертається чесно,
/// щоб виклик міг про це голосно сказати.
pub fn mirror_all(c: &Connection) -> Vec<MirrorOutcome> {
    mirror_targets()
        .into_iter()
        .map(|dest| match snapshot_to(c, &dest) {
            Ok(bytes) => MirrorOutcome { dest, bytes, error: None },
            Err(e) => MirrorOutcome { dest, bytes: 0, error: Some(e.to_string()) },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrate;

    fn tmpdir(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("wr-mirror-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn snapshot_is_a_real_readable_database() {
        let dir = tmpdir("snap");
        let mut src = Connection::open(dir.join("src.db")).unwrap();
        migrate(&mut src).unwrap();
        src.execute("INSERT INTO chains VALUES ('eth','Ethereum','ETH',NULL,1)", []).unwrap();

        let dest = dir.join("copy.db");
        let n = snapshot_to(&src, &dest).unwrap();
        assert!(n > 0);

        // копію треба ВІДКРИТИ й прочитати, а не лише перевірити розмір:
        // зіпсований файл теж має ненульовий розмір
        let c2 = Connection::open(&dest).unwrap();
        let got: String =
            c2.query_row("SELECT name FROM chains WHERE id='eth'", [], |r| r.get(0)).unwrap();
        assert_eq!(got, "Ethereum");
    }

    #[test]
    fn snapshot_overwrites_without_leaving_a_temp_file() {
        let dir = tmpdir("over");
        let mut src = Connection::open(dir.join("src.db")).unwrap();
        migrate(&mut src).unwrap();
        let dest = dir.join("copy.db");

        snapshot_to(&src, &dest).unwrap();
        src.execute("INSERT INTO chains VALUES ('base','Base','ETH',NULL,1)", []).unwrap();
        snapshot_to(&src, &dest).unwrap(); // другий раз, поверх наявного

        let c2 = Connection::open(&dest).unwrap();
        let n: i64 = c2.query_row("SELECT COUNT(*) FROM chains", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1, "другий знімок не побачив свіжий рядок");
        assert!(!dest.with_extension("db.tmp").exists(), "лишився тимчасовий файл");
    }

    #[test]
    fn relative_target_is_refused() {
        let dir = tmpdir("rel");
        let mut src = Connection::open(dir.join("src.db")).unwrap();
        migrate(&mut src).unwrap();
        // саме те, що сталось наживо: з'їдені слеші перетворюють шлях на відносний
        let e = snapshot_to(&src, Path::new("UsersSomeonewallet-radar-store/x.db"))
            .unwrap_err()
            .to_string();
        assert!(e.contains("абсолютний"), "відносний шлях прийнято: {e}");
    }

    #[test]
    fn unreachable_target_is_reported_not_panicked() {
        let dir = tmpdir("bad");
        let mut src = Connection::open(dir.join("src.db")).unwrap();
        migrate(&mut src).unwrap();
        // шлях на неіснуючому томі — імітація відключеного диска
        let r = snapshot_to(&src, Path::new("Q:\\nope\\x.db"));
        assert!(r.is_err(), "недоступна ціль не дала помилки");
    }
}
