//! Атомарний запис файлу: пишемо в сусідній тимчасовий файл, робимо fsync і
//! аж потім `rename` на місце. rename у межах однієї файлової системи атомарний,
//! тож будь-який читач бачить або старий вміст цілком, або новий цілком — ніколи
//! напів-записаний. Замінює розсипані по кодовій базі `std::fs::write`, через які
//! краш посеред запису (напр. 28-МБ `graph-db.json`) лишав пошкоджений файл.
//!
//! Сигнатура навмисно повторює `std::fs::write<P: AsRef<Path>, C: AsRef<[u8]>>`,
//! тож заміна на місцях виклику — суто текстова (`std::fs::write` →
//! `crate::atomicw::write`), без зміни поведінки, крім самої атомарності.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

// Робить ім'я тимчасового файлу унікальним у межах процесу, щоб два одночасні
// записи в один і той самий цільовий файл не наступали на спільний .tmp.
static SEQ: AtomicU64 = AtomicU64::new(0);

fn tmp_path(path: &Path) -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let mut s = path.as_os_str().to_owned();
    s.push(format!(".tmp.{}.{}", std::process::id(), n));
    PathBuf::from(s)
}

/// Атомарно записати `contents` у `path`. Помилки повертаються так само, як у
/// `std::fs::write` (виклики зазвичай роблять `let _ = ...`, як і раніше).
pub fn write<P: AsRef<Path>, C: AsRef<[u8]>>(path: P, contents: C) -> std::io::Result<()> {
    let path = path.as_ref();
    let tmp = tmp_path(path);

    // 1) повний запис у тимчасовий файл + fsync, щоб дані точно були на диску
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(contents.as_ref())?;
        f.sync_all()?;
    }

    // 2) атомарна підміна; при збої прибираємо тимчасовий файл, щоб не смітити
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_then_reads_back() {
        let dir = std::env::temp_dir().join(format!("atomicw-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let f = dir.join("x.json");
        write(&f, b"{\"a\":1}").unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "{\"a\":1}");
        // перезапис іншим вмістом — старий повністю замінено
        write(&f, b"{\"b\":2}").unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "{\"b\":2}");
        // жодних осиротілих .tmp у каталозі
        let leftover = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().contains(".tmp."));
        assert!(!leftover, "залишились тимчасові файли");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
