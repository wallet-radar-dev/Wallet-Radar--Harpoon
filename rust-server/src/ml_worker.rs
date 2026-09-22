//! Port of the WORKER half of server/ml-smart-money.js — the self-supervised training
//! loop behind the smart-money screener. The pure math (train / OOF / AUC / null
//! baseline) already lives in `ml.rs`; this is the state around it.
//!
//! What the loop does: take anchor addresses (whale clusters, cross-asset holders, alpha
//! signals, the richest crawler nodes), extract cheap structural features, label them by
//! REALIZED PnL, and retrain every 10 new samples.
//!
//! Two honesty rails, both load-bearing:
//!   - the model is RETROSPECTIVE. The label comes from the same snapshot as the features,
//!     so it answers "is this wallet historically profitable" — a cheap proxy for the
//!     expensive computeWalletScore, NOT a forecast.
//!   - so the forward track measures the thing people actually want: snapshot the features
//!     and PnL at T, and 14 days later ask whether the PnL GREW. Until ≥60 samples have
//!     matured we publish no forward number at all, because the confidence interval would
//!     be wider than any effect it could show.

use crate::jsmath::{js_num, js_round, round_fixed, to_fixed};
use crate::ml::{self, Sample};
use indexmap::{IndexMap, IndexSet};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// ml-smart-money.js:26-27
const SCHEMA: i64 = 2;
const MIN_SAMPLES: usize = 40;
const MAX_TRAIN: usize = 3000;
/// Retrain every N accepted samples.
pub const TRAIN_EVERY: u32 = 10;

/// The forward track (ml-smart-money.js:290-296).
// Мульти-горизонт forward-треку: кожен знімок оцінюється на цих горизонтах (дні).
// 365 = «1 рік». «Весь час» — не горизонт, а сукупний реалізований PnL (allTimePnlUsd нижче).
const FWD_HORIZONS: [i64; 5] = [1, 7, 14, 30, 365];
const FWD_MIN_MATURED: usize = 60;
const FWD_CAP: usize = 5000;
const DAY_MS: i64 = 86_400_000;

struct State {
    dir: PathBuf,
    /// [{addr, x, y, v, src, ts}]
    dataset: Vec<Value>,
    /// [{addr, x, ts, pnl0, y?, pnl1?, ts1?}]
    forward: Vec<Value>,
    /// addr → which anchor list it first came from.
    src: IndexMap<String, String>,
    /// Addresses already sampled — never re-queued.
    done: IndexSet<String>,
    queue: Vec<String>,
    since_train: u32,
    fwd_stats: Option<Value>,
}

fn state() -> &'static Mutex<State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    S.get_or_init(|| {
        Mutex::new(State {
            dir: PathBuf::from("."),
            dataset: vec![],
            forward: vec![],
            src: IndexMap::new(),
            done: IndexSet::new(),
            queue: vec![],
            since_train: 0,
            fwd_stats: None,
        })
    })
}

pub fn init(dataset_dir: &Path) {
    let read = |f: &str| -> Vec<Value> {
        std::fs::read_to_string(dataset_dir.join(f))
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default()
    };
    let mut s = state().lock().unwrap();
    s.dir = dataset_dir.to_path_buf();
    // Samples of an older feature schema are simply not usable by the current model.
    s.dataset = read("sm-train.json").into_iter().filter(|d| d["v"].as_i64() == Some(SCHEMA)).collect();
    s.forward = read("sm-forward.json");
    s.done = s.dataset.iter().filter_map(|d| d["addr"].as_str().map(String::from)).collect();
}

pub fn dataset_len() -> usize {
    state().lock().unwrap().dataset.len()
}

/// ml-smart-money.js `seeds()` — the anchor addresses, each tagged with the list it first
/// appeared in. The source matters: it was the DIFFERENT positive rate per source that
/// made the old graph features look like a signal, so it is recorded explicitly and can
/// be audited.
pub fn seeds(clusters: &Value, cross: &Value, alpha: &Value, graph: &Value) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut seen: IndexSet<String> = IndexSet::new();
    let mut add = |a: &str, src: &str, out: &mut Vec<(String, String)>| {
        let a = a.to_lowercase();
        let ok = a.len() == 42
            && a.starts_with("0x")
            && a[2..].bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if ok && seen.insert(a.clone()) {
            out.push((a, src.to_string()));
        }
    };
    let empty = vec![];
    for c in clusters["clusters"].as_array().unwrap_or(&empty) {
        for m in c["members"].as_array().unwrap_or(&empty) {
            if let Some(a) = m["addr"].as_str() {
                add(a, "whale-cluster", &mut out);
            }
        }
    }
    for p in cross["multiAsset"].as_array().unwrap_or(&empty) {
        if let Some(a) = p["address"].as_str() {
            add(a, "cross-asset", &mut out);
        }
    }
    for a in alpha.as_array().unwrap_or(&empty) {
        if let Some(addr) = a["address"].as_str() {
            add(addr, "alpha-feed", &mut out);
        }
    }
    // The 200 richest crawler nodes by USD flow.
    if let Some(nodes) = graph["nodes"].as_object() {
        let mut top: Vec<(&String, f64)> = nodes.iter().map(|(a, n)| (a, n["v"].as_f64().unwrap_or(0.0))).collect();
        top.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        for (a, _) in top.into_iter().take(200) {
            add(a, "graph-top", &mut out);
        }
    }
    out
}

/// ml-smart-money.js `labelOf` — SMART = net realized PnL > 0. A wallet with fewer than
/// two closed trades is not labelled at all: it has no track record to be right about.
pub fn label_of(score: &Value) -> Option<(u8, f64)> {
    if score["closed"].as_f64().unwrap_or(0.0) < 2.0 {
        return None;
    }
    let pnl = score["realizedUsd"].as_f64()?;
    Some((if pnl > 0.0 { 1 } else { 0 }, pnl))
}

/// Refill the queue from the anchors, skipping addresses already sampled.
pub fn refill_queue(seeds: Vec<(String, String)>) {
    let mut s = state().lock().unwrap();
    if !s.queue.is_empty() {
        return;
    }
    for (a, src) in seeds {
        s.src.entry(a.clone()).or_insert(src);
        if !s.done.contains(&a) {
            s.queue.push(a);
        }
    }
}

pub fn next_addr() -> Option<String> {
    let mut s = state().lock().unwrap();
    if s.queue.is_empty() {
        return None;
    }
    let a = s.queue.remove(0);
    s.done.insert(a.clone());
    Some(a)
}

/// Record one labelled sample. Returns true when it is time to retrain.
pub fn push_sample(addr: &str, x: &[f64], y: u8, now: i64) -> bool {
    let mut s = state().lock().unwrap();
    let src = s.src.get(addr).cloned().unwrap_or_else(|| "unknown".into());
    s.dataset.push(json!({
        "addr": addr, "x": x, "y": js_num(y as f64), "v": js_num(SCHEMA as f64), "src": src, "ts": js_num(now as f64),
    }));
    if s.dataset.len() > MAX_TRAIN {
        s.dataset.remove(0);
    }
    let text = serde_json::to_string(&Value::Array(s.dataset.clone()));
    let file = s.dir.join("sm-train.json");
    if let Ok(t) = text {
        let _ = crate::atomicw::write(file, t);
    }
    s.since_train += 1;
    if s.since_train >= TRAIN_EVERY {
        s.since_train = 0;
        return true;
    }
    false
}

/// Заморозити фічі+PnL у T для forward-тесту. Один активний знімок на адресу, доки не
/// визрів «блокуючий» 30-денний горизонт (365-денна мітка добереться на тому ж знімку,
/// доки він у буфері) — інакше та сама адреса домінувала б у вибірці.
/// `m` — мапа горизонт(рядок)->{y,pnl1,ts1}, заповнюється поступово.
pub fn snapshot_forward(addr: &str, x: &[f64], pnl0: f64, now: i64) {
    if !pnl0.is_finite() {
        return;
    }
    let mut s = state().lock().unwrap();
    if s.forward.iter().any(|f| f["addr"].as_str() == Some(addr) && f["m"]["30"].is_null()) {
        return;
    }
    s.forward.push(json!({"addr": addr, "x": x, "ts": js_num(now as f64), "pnl0": js_num(pnl0), "m": {}}));
    if s.forward.len() > FWD_CAP {
        s.forward.remove(0);
    }
    save_forward(&s);
}

fn save_forward(s: &std::sync::MutexGuard<State>) {
    if let Ok(t) = serde_json::to_string(&Value::Array(s.forward.clone())) {
        let _ = crate::atomicw::write(s.dir.join("sm-forward.json"), t);
    }
}

/// Знімки, що перетнули якийсь горизонт, але ще не мають його мітки — до 20 за прохід.
/// Повертає (addr, idx, horizon_days). Один знімок може дати кілька записів (різні горизонти).
pub fn due_forward(now: i64) -> Vec<(String, usize, i64)> {
    let s = state().lock().unwrap();
    let mut out: Vec<(String, usize, i64)> = Vec::new();
    for (i, f) in s.forward.iter().enumerate() {
        let ts = f["ts"].as_i64().unwrap_or(0);
        let Some(addr) = f["addr"].as_str() else { continue };
        for &h in FWD_HORIZONS.iter() {
            if f["m"][h.to_string()].is_null() && now - ts >= h * DAY_MS {
                out.push((addr.to_string(), i, h));
                if out.len() >= 20 {
                    return out;
                }
            }
        }
    }
    out
}

/// Мітка одного горизонту: чи гаманець ЗАРОБИВ за цей горизонт (не «чи був колись у плюсі»).
pub fn mature_one(idx: usize, horizon: i64, realized_usd: f64, now: i64) {
    let mut s = state().lock().unwrap();
    let Some(f) = s.forward.get_mut(idx) else { return };
    let pnl0 = f["pnl0"].as_f64().unwrap_or(0.0);
    f["m"][horizon.to_string()] = json!({
        "y": js_num(if realized_usd - pnl0 > 0.0 { 1.0 } else { 0.0 }),
        "pnl1": js_num(realized_usd),
        "ts1": js_num(now as f64),
    });
    save_forward(&s);
}

fn samples_of(rows: &[Value]) -> Vec<Sample> {
    rows.iter()
        .filter_map(|d| {
            let x: Vec<f64> = d["x"].as_array()?.iter().map(|v| v.as_f64().unwrap_or(0.0)).collect();
            Some(Sample { x, y: d["y"].as_u64()? as u8 })
        })
        .collect()
}

/// Per-горизонт статистика forward-треку + сукупний (весь час) реалізований PnL.
/// Кожен горизонт публікує AUC окремо, щойно набере ≥FWD_MIN_MATURED визрілих на ньому;
/// доти показує matured/pending і auc=null (та сама чесна рейка, але per-горизонт).
/// Повертає Some, лише коли хоч один горизонт опублікувався (для логу); fwd_stats
/// оновлюється завжди, тож /api/ml-forward бачить свіжий per-горизонтний зріз.
pub fn recompute_forward_stats() -> Option<Value> {
    let mut s = state().lock().unwrap();
    let mut horizons = serde_json::Map::new();
    let mut any_published = false;
    for &h in FWD_HORIZONS.iter() {
        let key = h.to_string();
        // семпли з міткою цього горизонту; «плоский» y, щоб перевикористати ml::
        let matured: Vec<Value> = s
            .forward
            .iter()
            .filter(|f| !f["m"][&key].is_null())
            .map(|f| {
                let mut g = f.clone();
                g["y"] = f["m"][&key]["y"].clone();
                g
            })
            .collect();
        let pending = s.forward.len().saturating_sub(matured.len());
        if matured.len() < FWD_MIN_MATURED {
            horizons.insert(key, json!({
                "matured": js_num(matured.len() as f64),
                "pending": js_num(pending as f64),
                "auc": Value::Null,
                "note": format!("потрібно ≥{FWD_MIN_MATURED}"),
            }));
            continue;
        }
        let data = samples_of(&matured);
        let pos = matured.iter().filter(|f| f["y"].as_u64() == Some(1)).count();
        let pairs = ml::oof_pairs(&data, 5);
        let nul = ml::null_baseline(&data, 5, 5);
        let (auc, acc) = match &pairs {
            Some(p) => {
                any_published = true;
                (
                    ml::auc_from(p).map_or(Value::Null, |a| js_num(round_fixed(a, 3))),
                    js_num(js_round(ml::bal_acc_from(p) * 100.0)),
                )
            }
            None => (Value::Null, Value::Null),
        };
        horizons.insert(key, json!({
            "matured": js_num(matured.len() as f64),
            "pending": js_num(pending as f64),
            "auc": auc,
            "acc": acc,
            "nullAuc": nul.map_or(Value::Null, |(_, a)| js_num(round_fixed(a, 3))),
            "nullAcc": nul.map_or(Value::Null, |(a, _)| js_num(js_round(a * 100.0))),
            "posRate": js_num(js_round((pos as f64 / matured.len() as f64) * 100.0)),
        }));
    }
    let stats = json!({
        "horizons": Value::Object(horizons),
        "allTimePnlUsd": js_num(round_fixed(all_time_pnl(&s.forward), 2)),
        "samples": js_num(s.forward.len() as f64),
    });
    s.fwd_stats = Some(stats.clone());
    if any_published {
        Some(stats)
    } else {
        None
    }
}

/// «Весь час»: сума останніх відомих реалізацій PnL по кожному знімку (найдовший визрілий
/// pnl1, інакше pnl0 на момент знімка). Це не forward-горизонт, а сукупний реалізований PnL.
fn all_time_pnl(forward: &[Value]) -> f64 {
    let mut sum = 0.0;
    for f in forward {
        let mut latest = f["pnl0"].as_f64();
        for &h in FWD_HORIZONS.iter() {
            if let Some(p) = f["m"][h.to_string()]["pnl1"].as_f64() {
                latest = Some(p);
            }
        }
        sum += latest.unwrap_or(0.0);
    }
    sum
}

/// `/api/ml-forward` payload — per-горизонтний зріз (1/7/14/30/365 днів) + allTimePnlUsd.
/// Кожен горизонт чесно каже auc=null, доки не набере ≥FWD_MIN_MATURED визрілих.
pub fn forward_status() -> Value {
    let s = state().lock().unwrap();
    if let Some(st) = &s.fwd_stats {
        return st.clone();
    }
    let mut horizons = serde_json::Map::new();
    for &h in FWD_HORIZONS.iter() {
        let key = h.to_string();
        let matured = s.forward.iter().filter(|f| !f["m"][&key].is_null()).count();
        horizons.insert(key, json!({
            "matured": js_num(matured as f64),
            "pending": js_num(s.forward.len().saturating_sub(matured) as f64),
            "auc": Value::Null,
            "note": format!("потрібно ≥{FWD_MIN_MATURED} визрілих семплів"),
        }));
    }
    json!({
        "horizons": Value::Object(horizons),
        "allTimePnlUsd": js_num(round_fixed(all_time_pnl(&s.forward), 2)),
        "samples": js_num(s.forward.len() as f64),
    })
}

/// ml-smart-money.js `retrain` — train, evaluate honestly, and write sm-model.json.
/// Returns the log line, or None if there is not enough data (or not enough of BOTH
/// classes) to train on.
pub fn retrain(now: i64) -> Option<String> {
    let s = state().lock().unwrap();
    let pos = s.dataset.iter().filter(|d| d["y"].as_u64() == Some(1)).count();
    let neg = s.dataset.len() - pos;
    if s.dataset.len() < MIN_SAMPLES || pos < 3 || neg < 3 {
        return None;
    }
    let data = samples_of(&s.dataset);
    let m = ml::train(&data);
    let ev = ml::evaluate(&data, &m);

    let model = json!({
        "w": m.w, "b": m.b, "mean": m.mean, "std": m.std,
        "feats": crate::smart_money::FEATS,
        "schema": js_num(SCHEMA as f64),
        "acc": js_num(ev.in_sample as f64),
        "cvAcc": ev.cv_acc.map_or(Value::Null, |v| js_num(v as f64)),
        "cvAuc": ev.cv_auc.map_or(Value::Null, |v| js_num(v)),
        "nullAcc": ev.null_acc.map_or(Value::Null, |v| js_num(v as f64)),
        "nullAuc": ev.null_auc.map_or(Value::Null, |v| js_num(v)),
        "active": ev.active,
        "samples": js_num(s.dataset.len() as f64),
        "posRate": js_num(js_round((pos as f64 / s.dataset.len() as f64) * 100.0)),
        "updatedAt": js_num(now as f64),
    });
    if let Ok(t) = serde_json::to_string(&model) {
        let _ = crate::atomicw::write(s.dir.join("sm-model.json"), t);
    }
    let fmt = |v: Option<f64>| v.map_or("—".to_string(), |x| to_fixed(x, 3));
    let fmti = |v: Option<i64>| v.map_or("—".to_string(), |x| x.to_string());
    Some(format!(
        "перенавчено: {} прикладів ({pos}+/{neg}-) · CV {}% / AUC {} · нульова лінія {}% / AUC {} · {}",
        s.dataset.len(),
        fmti(ev.cv_acc),
        fmt(ev.cv_auc),
        fmti(ev.null_acc),
        fmt(ev.null_auc),
        if ev.active { "АКТИВНА" } else { "НЕАКТИВНА (не б'є нульову лінію)" }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeds_tag_each_address_with_the_list_it_first_appeared_in() {
        let a = "0xaaaa000000000000000000000000000000000001";
        let b = "0xbbbb000000000000000000000000000000000002";
        let clusters = json!({"clusters": [{"members": [{"addr": a}]}]});
        // `a` shows up again in a later list — the FIRST source must stick, or the
        // per-source positive rates we audit become meaningless.
        let cross = json!({"multiAsset": [{"address": a}, {"address": b}]});
        let alpha = json!([]);
        let graph = json!({"nodes": {}});

        let s = seeds(&clusters, &cross, &alpha, &graph);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0], (a.to_string(), "whale-cluster".to_string()));
        assert_eq!(s[1], (b.to_string(), "cross-asset".to_string()));
    }

    #[test]
    fn seeds_take_the_richest_crawler_nodes_and_reject_malformed_addresses() {
        let rich = "0x1111111111111111111111111111111111111111";
        let poor = "0x2222222222222222222222222222222222222222";
        let graph = json!({"nodes": {poor: {"v": 1.0}, rich: {"v": 1e9}, "notanaddress": {"v": 1e12}}});
        let s = seeds(&json!({}), &json!({}), &json!([]), &graph);

        assert_eq!(s[0].0, rich, "richest first");
        assert_eq!(s.len(), 2, "the malformed key is dropped");
        assert!(s.iter().all(|(_, src)| src == "graph-top"));
    }

    #[test]
    fn a_wallet_with_fewer_than_two_closed_trades_is_not_labelled() {
        assert_eq!(label_of(&json!({"closed": 1, "realizedUsd": 5000.0})), None);
        assert_eq!(label_of(&json!({"closed": 5, "realizedUsd": null})), None);

        assert_eq!(label_of(&json!({"closed": 2, "realizedUsd": 1.0})), Some((1, 1.0)));
        assert_eq!(label_of(&json!({"closed": 9, "realizedUsd": -1.0})), Some((0, -1.0)));
        // Exactly break-even is NOT smart: the JS tests `> 0`.
        assert_eq!(label_of(&json!({"closed": 9, "realizedUsd": 0.0})), Some((0, 0.0)));
    }
}
