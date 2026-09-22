//! Whale-family membership lookup. A transfer whose two ends belong to the SAME
//! family (funding-graph component from families.rs) is internal redistribution —
//! a whale shuffling into its own pre-pump/pump wallets — NOT a market signal.
//! Also powers the wallet→family link in the UI.
//!
//! Backed by dataset/families-map.csv (wallet,family), cached 10 min so a fresh
//! `families` run is picked up without a restart.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

static CACHE: Mutex<Option<(Instant, HashMap<String, String>)>> = Mutex::new(None);

fn load(root: &Path) -> HashMap<String, String> {
    let mut m = HashMap::new();
    if let Ok(txt) = std::fs::read_to_string(root.join("families-map.csv")) {
        for line in txt.lines().skip(1) {
            if let Some((w, fam)) = line.split_once(',') {
                m.insert(w.trim().to_lowercase(), fam.trim().to_string());
            }
        }
    }
    m
}

fn with_map<R>(root: &Path, f: impl FnOnce(&HashMap<String, String>) -> R) -> R {
    let mut guard = CACHE.lock().unwrap();
    let stale = guard
        .as_ref()
        .map(|(t, _)| t.elapsed() > Duration::from_secs(600))
        .unwrap_or(true);
    if stale {
        *guard = Some((Instant::now(), load(root)));
    }
    f(&guard.as_ref().unwrap().1)
}

/// True iff `a` and `b` are both in the graph AND in the same family.
pub fn same_family(root: &Path, a: &str, b: &str) -> bool {
    if a.is_empty() || b.is_empty() {
        return false;
    }
    with_map(root, |m| {
        match (m.get(&a.to_lowercase()), m.get(&b.to_lowercase())) {
            (Some(fa), Some(fb)) => fa == fb,
            _ => false,
        }
    })
}

/// The family id (top-volume member address) this wallet belongs to, if any.
pub fn family_of(root: &Path, a: &str) -> Option<String> {
    if a.is_empty() {
        return None;
    }
    with_map(root, |m| m.get(&a.to_lowercase()).cloned())
}
