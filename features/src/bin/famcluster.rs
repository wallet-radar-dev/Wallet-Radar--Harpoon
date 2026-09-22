//! Family archetype clustering — the "train AI on GROUPS" step. k-means over the
//! enriched families' feature vectors (size, volume, in/out ratio, emission
//! concentration, dominant sector) → behavioral archetypes. Each archetype gets
//! a human label from its members' modal phase + modal sector + size tier.
//! Writes `archetype` (index) + `archetypeLabel` per family, and an `archetypes`
//! summary, into families.json.
//!
//! In/Out: dataset/families.json (enriched by famfeatures). Local-only.

use std::collections::HashMap;

use serde_json::{json, Value};

const SECS: [&str; 5] = ["depin", "memecoin", "defi", "rwa", "other"];
const K: usize = 5;

fn sector_idx(s: &str) -> usize {
    SECS.iter().position(|&x| x == s).unwrap_or(4)
}

/// deterministic pseudo-seed spread of initial centroids (no RNG — reproducible).
fn kmeans(x: &[Vec<f64>], k: usize, iters: usize) -> Vec<usize> {
    let n = x.len();
    let dim = x[0].len();
    // init: pick k points spread across the set by stride
    let mut cent: Vec<Vec<f64>> = (0..k).map(|c| x[(c * n) / k].clone()).collect();
    let mut assign = vec![0usize; n];
    for _ in 0..iters {
        // assign
        for (i, p) in x.iter().enumerate() {
            let mut best = 0;
            let mut bd = f64::MAX;
            for (c, ce) in cent.iter().enumerate() {
                let d: f64 = p.iter().zip(ce).map(|(a, b)| (a - b) * (a - b)).sum();
                if d < bd {
                    bd = d;
                    best = c;
                }
            }
            assign[i] = best;
        }
        // update
        let mut sums = vec![vec![0.0; dim]; k];
        let mut cnt = vec![0usize; k];
        for (i, p) in x.iter().enumerate() {
            let c = assign[i];
            cnt[c] += 1;
            for d in 0..dim {
                sums[c][d] += p[d];
            }
        }
        for c in 0..k {
            if cnt[c] > 0 {
                for d in 0..dim {
                    cent[c][d] = sums[c][d] / cnt[c] as f64;
                }
            }
        }
    }
    assign
}

fn main() {
    let root = std::env::var("APP_ROOT").unwrap_or_else(|_| "/opt/wallet-tracker".into());
    let fp = format!("{root}/dataset/families.json");
    let mut doc: Value = serde_json::from_str(&std::fs::read_to_string(&fp).expect("families.json")).unwrap();

    // collect enriched families + raw features
    let fams = doc["families"].as_array().cloned().unwrap_or_default();
    let mut idx: Vec<usize> = Vec::new();
    let mut raw: Vec<Vec<f64>> = Vec::new();
    for (i, f) in fams.iter().enumerate() {
        if f["transfers"].as_u64().unwrap_or(0) == 0 {
            continue;
        }
        let size = (f["size"].as_f64().unwrap_or(1.0)).max(1.0).log10();
        let vol = (f["volumeUsd"].as_f64().unwrap_or(1.0)).max(1.0).log10();
        let inr = f["inRatio"].as_f64().unwrap_or(50.0) / 100.0;
        let emax = f["topEmission"].get(0).and_then(|e| e["emissionPct"].as_f64()).unwrap_or(0.0) / 100.0;
        let dsec = f["dominantSector"].as_str().map(sector_idx).unwrap_or(4) as f64 / 4.0;
        idx.push(i);
        raw.push(vec![size, vol, inr, emax, dsec]);
    }
    if raw.len() < K {
        eprintln!("[famcluster] лише {} збагачених родин (<{}), пропускаю", raw.len(), K);
        return;
    }

    // min-max normalize each column
    let dim = raw[0].len();
    let mut x = raw.clone();
    for d in 0..dim {
        let (mn, mx) = raw.iter().fold((f64::MAX, f64::MIN), |(a, b), r| (a.min(r[d]), b.max(r[d])));
        let rng = (mx - mn).max(1e-9);
        for r in x.iter_mut() {
            r[d] = (r[d] - mn) / rng;
        }
    }

    let assign = kmeans(&x, K, 40);

    // label each cluster from members' modal phase + modal sector + size tier
    let mut labels: Vec<String> = vec![String::new(); K];
    let mut counts = vec![0usize; K];
    for c in 0..K {
        let members: Vec<usize> = (0..idx.len()).filter(|&j| assign[j] == c).collect();
        counts[c] = members.len();
        if members.is_empty() {
            labels[c] = "—".into();
            continue;
        }
        let modal = |key: &str| -> String {
            let mut m: HashMap<String, usize> = HashMap::new();
            for &j in &members {
                if let Some(v) = fams[idx[j]][key].as_str() {
                    *m.entry(v.to_string()).or_default() += 1;
                }
            }
            m.into_iter().max_by_key(|(_, n)| *n).map(|(k, _)| k).unwrap_or_default()
        };
        let phase = modal("phase");
        let sector = modal("dominantSector");
        let avg_size: f64 = members.iter().map(|&j| fams[idx[j]]["size"].as_f64().unwrap_or(0.0)).sum::<f64>() / members.len() as f64;
        let tier = if avg_size >= 200.0 { "мега" } else if avg_size >= 30.0 { "велика" } else { "мала" };
        let ph = match phase.as_str() { "accumulating" => "набирає", "distributing" => "роздає", _ => "нейтр." };
        let sec = if sector.is_empty() { "".to_string() } else { format!(" {sector}") };
        labels[c] = format!("{tier} · {ph}{sec}");
    }

    // write back
    if let Some(arr) = doc["families"].as_array_mut() {
        for (j, &i) in idx.iter().enumerate() {
            arr[i]["archetype"] = json!(assign[j]);
            arr[i]["archetypeLabel"] = json!(labels[assign[j]]);
        }
    }
    let summary: Vec<Value> = (0..K).map(|c| json!({"id": c, "label": labels[c], "count": counts[c]})).collect();
    doc["archetypes"] = json!(summary);
    std::fs::write(&fp, serde_json::to_string(&doc).unwrap()).unwrap();
    eprintln!(
        "[famcluster] {} родин у {} архетипів: {}",
        idx.len(), K,
        (0..K).map(|c| format!("[{}:{}]", labels[c], counts[c])).collect::<Vec<_>>().join(" ")
    );
}
