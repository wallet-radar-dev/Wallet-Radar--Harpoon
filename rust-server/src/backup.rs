//! Port of server/backup.js — a daily copy of the dataset into
//! `dataset/.backups/<YYYY-MM-DD>/`, keeping the last 7 days. Cheap insurance
//! against a disk failure or an accidental delete.

use std::path::Path;

/// backup.js SKIP — regenerated and large, not worth backing up.
const SKIP: [&str; 2] = ["cache.json", "labels-cache.json"];
const KEEP: usize = 7;

/// Which day directories to delete, given every `YYYY-MM-DD` dir present (any
/// order) and how many to keep. Oldest first — the JS shifts off the sorted front.
fn stale_days(mut days: Vec<String>, keep: usize) -> Vec<String> {
    days.retain(|d| is_day(d));
    days.sort();
    if days.len() <= keep {
        return vec![];
    }
    days.truncate(days.len() - keep);
    days
}

/// `/^\d{4}-\d{2}-\d{2}$/`
fn is_day(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter().enumerate().all(|(i, c)| matches!(i, 4 | 7) || c.is_ascii_digit())
}

/// backup.js `backupOnce` — copy today's dataset, then rotate.
pub fn backup_once(dataset_dir: &Path, data_dir: &Path, day: &str) {
    let root = dataset_dir.join(".backups");
    let dir = root.join(day);
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }

    let mut n = 0usize;
    if let Ok(entries) = std::fs::read_dir(dataset_dir) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if !name.ends_with(".json") || SKIP.contains(&name.as_str()) {
                continue;
            }
            if std::fs::copy(e.path(), dir.join(&name)).is_ok() {
                n += 1;
            }
        }
    }
    // data.json (the watchlist + events) lives in DATA_DIR, outside the dataset.
    let data_file = data_dir.join("data.json");
    if data_file.exists() && std::fs::copy(&data_file, dir.join("data.json")).is_ok() {
        n += 1;
    }

    let days: Vec<String> = std::fs::read_dir(&root)
        .map(|es| es.flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect())
        .unwrap_or_default();
    for old in stale_days(days, KEEP) {
        let _ = std::fs::remove_dir_all(root.join(old));
    }
    tracing::info!("[backup] saved {n} files → dataset/.backups/{day} (keeping {KEEP} days)");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_keeps_the_newest_seven() {
        let days: Vec<String> = (1..=10).map(|d| format!("2026-07-{d:02}")).collect();
        let stale = stale_days(days, KEEP);
        // 10 days present, keep 7 → the 3 oldest go.
        assert_eq!(stale, vec!["2026-07-01", "2026-07-02", "2026-07-03"]);
    }

    #[test]
    fn rotation_is_a_no_op_below_the_limit_and_ignores_stray_dirs() {
        let days = vec!["2026-07-01".to_string(), "2026-07-02".to_string()];
        assert!(stale_days(days, KEEP).is_empty());

        // Anything that is not a YYYY-MM-DD directory is left alone entirely.
        let mixed: Vec<String> = (1..=9)
            .map(|d| format!("2026-07-{d:02}"))
            .chain(["tmp".to_string(), "2026-7-1".to_string(), "readme.md".to_string()])
            .collect();
        let stale = stale_days(mixed, KEEP);
        assert_eq!(stale, vec!["2026-07-01", "2026-07-02"]);
    }
}
