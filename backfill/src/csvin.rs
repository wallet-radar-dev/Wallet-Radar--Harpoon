//! Прозоре читання великих CSV, старша частина яких заархівована в `.zst`.
//!
//! Навіщо: `transfers-grp.csv` виріс до 138 GiB, `transfers-native.csv` — до 34 GiB,
//! і вдвох вони займали 96% диска даних (216 GiB). Місця лишалося менш ніж на дві доби.
//! Ці CSV стискаються zstd у 7.3 раза, тож архівування звільняє ~149 GiB БЕЗ втрати
//! жодного рядка — але тільки якщо кожен читач бачить і архіви, і живий файл.
//!
//! Схема іменування: живий файл `transfers-grp.csv` дописується збирачем як і раніше,
//! а прокручені шматки лежать поруч як `transfers-grp.csv.001.zst`, `.002.zst`, …
//! Збирачам (`backfill2`, `ethflow`) знати про архіви не треба — вони лише дописують.
//!
//! ГОЛОСНА ВІДМОВА. Якщо архів не відкривається чи не розтискається, ми НЕ пропускаємо
//! його мовчки: аналіз на половині даних виглядає як успішний і дає тихо неправильні
//! числа. Це та сама пастка, що вже коштувала проєкту хибних висновків (див. коментарі
//! про NOTOK і про обрив розбору на непридатному байті). Тому — паніка з ясним текстом.

use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;

/// Той самий буфер, що використовували читачі до появи цього модуля.
const CAP: usize = 1 << 22;

/// Обгортка, що перетворює помилку читання на паніку.
///
/// Без неї «голосна відмова» не працює, і це перевірено дослідом: пошкодження в
/// СЕРЕДИНІ архіву не заважає ні `File::open`, ні `Decoder::new` — вони проходять
/// успішно, а помилка вилазить лише під час читання. Читачі ж помилки читання
/// ковтають (`map_while(Result::ok)`, `Err(_) => continue`), тому бін мовчки
/// обробляв обрізок: у тесті з 372 рядків він прочитав 70 і вийшов з кодом 0,
/// оголосивши 81% гаманців неактивними. Тепер такий архів валить прогін.
struct Strict<R: Read> {
    inner: R,
    what: String,
}

impl<R: Read> Read for Strict<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self.inner.read(buf) {
            Ok(n) => Ok(n),
            Err(e) => panic!(
                "[csvin] архів {} читається з помилкою: {e}. \
                 Далі був би аналіз на обрізку даних — зупиняюсь.",
                self.what
            ),
        }
    }
}

/// Читачі для `path` та всіх його архівів, у порядку: найстаріший архів → … → живий файл.
///
/// Порожній вектор означає, що немає НІ живого файлу, НІ архівів — тобто джерела справді
/// нема. Викличник має відрізняти це від «файл є, але порожній».
pub fn readers(path: &str) -> Vec<Box<dyn BufRead>> {
    let mut out: Vec<Box<dyn BufRead>> = Vec::new();
    let p = std::path::Path::new(path);
    let (Some(dir), Some(base)) = (p.parent(), p.file_name()) else {
        return out;
    };
    let base = base.to_string_lossy().to_string();
    let prefix = format!("{base}.");

    // Архіви: <base>.<номер>.zst. Номер — щоб порядок був передбачуваний, а не за
    // примхою read_dir (він не сортує).
    let mut arch: Vec<(u32, PathBuf)> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let Some(rest) = name.strip_prefix(&prefix) else { continue };
            let Some(num) = rest.strip_suffix(".zst") else { continue };
            let Ok(n) = num.parse::<u32>() else { continue };
            arch.push((n, e.path()));
        }
    }
    arch.sort_by_key(|(n, _)| *n);

    for (_, ap) in arch {
        let f = File::open(&ap).unwrap_or_else(|e| {
            panic!("[csvin] архів {} не відкривається: {e}. \
                    Читати далі не можна — вийде аналіз на неповних даних.", ap.display())
        });
        let d = zstd::stream::read::Decoder::new(f).unwrap_or_else(|e| {
            panic!("[csvin] архів {} не розтискається: {e}. \
                    Читати далі не можна — вийде аналіз на неповних даних.", ap.display())
        });
        eprintln!("[csvin] архів {}", ap.display());
        let strict = Strict { inner: d, what: ap.display().to_string() };
        out.push(Box::new(BufReader::with_capacity(CAP, strict)));
    }

    // Живий файл — останнім: у ньому найсвіжіші рядки.
    if let Ok(f) = File::open(path) {
        out.push(Box::new(BufReader::with_capacity(CAP, f)));
    }
    out
}
