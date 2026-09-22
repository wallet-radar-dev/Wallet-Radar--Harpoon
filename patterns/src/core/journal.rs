//! ЖУРНАЛ ПЕРЕРАХУНКІВ (Фаза 0.3 + X.2 плану): коли, яким КОДОМ, на яких ДАНИХ побудовано вітрину.
//! Кожен інструмент, що пише вітрину, дописує рядок у `recompute-journal.jsonl`:
//!   ts, tool, code_hash (fingerprint білда), dict-версія, вихід, входи (файл+mtime+size).
//! Так по БУДЬ-ЯКІЙ цифрі можна сказати: звідки вона, коли й яким кодом зроблена (відтворюваність доказова).
use std::io::Write;
use serde_json::json;
use std::sync::OnceLock;

static HASH: OnceLock<String> = OnceLock::new();

/// Fingerprint поточного виконуваного файлу (FNV-1a 64) — ідентифікує БІЛД коду.
/// Кешується: рахуємо раз на процес (читання бінарника недешеве).
pub fn code_hash() -> &'static str {
    HASH.get_or_init(|| {
        let mut h: u64 = 0xcbf29ce484222325;
        if let Ok(exe) = std::env::current_exe() {
            if let Ok(bytes) = std::fs::read(&exe) {
                for b in bytes { h ^= b as u64; h = h.wrapping_mul(0x100000001b3); }
            }
        }
        format!("{h:016x}")
    })
}

fn file_meta(path: &str) -> serde_json::Value {
    let m = std::fs::metadata(path).ok();
    let mtime = m.as_ref().and_then(|x| x.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
    json!({"f": path.rsplit('/').next().unwrap_or(path), "size": m.as_ref().map(|x| x.len()).unwrap_or(0), "mtime": mtime})
}

/// Дописати запис у журнал перерахунків теки `dir`.
pub fn record(dir: &str, tool: &str, out: &str, inputs: &[&str]) {
    let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs()).unwrap_or(0);
    let ins: Vec<serde_json::Value> = inputs.iter().map(|f| file_meta(f)).collect();
    let entry = json!({
        "ts": ts, "tool": tool, "code_hash": code_hash(), "dict": wr_dict::dict_version(),
        "out": out.rsplit('/').next().unwrap_or(out), "inputs": ins
    });
    let path = format!("{dir}/recompute-journal.jsonl");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{entry}");
    }
}
