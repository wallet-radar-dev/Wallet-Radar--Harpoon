//! K-means clustering of the per-wallet feature table into behavioural archetypes.
//! Pure std. Log-transforms skewed counts, z-score standardizes, k-means++ init
//! with a seeded PRNG (reproducible), best-of-N restarts.
//!
//! In:  dataset/backfill/features.csv
//! Out: dataset/backfill/clusters.csv  (wallet,cluster)  + printed cluster profiles.

use std::io::{BufRead, BufReader, BufWriter, Write};

struct Xorshift(u64);
impl Xorshift {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

const FEATS: [&str; 12] = [
    "n", "n_tokens", "active_days", "lifespan_days", "tx_per_active_day", // log1p
    "in_ratio", "stable_frac", "primary_chain_share", "chain_entropy", "token_entropy",
    "n_chains", "top_token_share",
];
const LOG: usize = 5; // first LOG features get log1p

fn main() {
    let root = std::env::var("APP_ROOT").unwrap_or_else(|_| "/opt/wallet-tracker".into());
    let k: usize = std::env::var("K").ok().and_then(|v| v.parse().ok()).unwrap_or(6);
    let inp = format!("{root}/dataset/backfill/features.csv");
    let outp = format!("{root}/dataset/backfill/clusters.csv");

    let f = std::fs::File::open(&inp).expect("open features.csv");
    let rdr = BufReader::new(f);
    let mut lines = rdr.lines();
    let header: Vec<String> = lines.next().unwrap().unwrap().split(',').map(String::from).collect();
    let idx = |name: &str| header.iter().position(|h| h == name).unwrap();
    let col: Vec<usize> = FEATS.iter().map(|f| idx(f)).collect();
    let iw = idx("wallet");

    let mut wallets: Vec<String> = Vec::new();
    let mut x: Vec<Vec<f64>> = Vec::new(); // transformed feature rows
    let mut raw: Vec<Vec<f64>> = Vec::new(); // original values (for profiling)
    for line in lines {
        let line = line.unwrap();
        let p: Vec<&str> = line.split(',').collect();
        if p.len() < header.len() {
            continue;
        }
        let mut row = Vec::with_capacity(FEATS.len());
        let mut rr = Vec::with_capacity(FEATS.len());
        for (j, &c) in col.iter().enumerate() {
            let v: f64 = p[c].parse().unwrap_or(0.0);
            rr.push(v);
            row.push(if j < LOG { (1.0 + v).ln() } else { v });
        }
        wallets.push(p[iw].to_string());
        x.push(row);
        raw.push(rr);
    }
    let n = x.len();
    let d = FEATS.len();

    // z-score standardize
    let mut mean = vec![0.0; d];
    let mut std = vec![0.0; d];
    for row in &x {
        for j in 0..d {
            mean[j] += row[j];
        }
    }
    for m in mean.iter_mut() {
        *m /= n as f64;
    }
    for row in &x {
        for j in 0..d {
            let dv = row[j] - mean[j];
            std[j] += dv * dv;
        }
    }
    for s in std.iter_mut() {
        *s = (*s / n as f64).sqrt();
        if *s < 1e-9 {
            *s = 1.0;
        }
    }
    for row in x.iter_mut() {
        for j in 0..d {
            row[j] = (row[j] - mean[j]) / std[j];
        }
    }

    let dist2 = |a: &[f64], b: &[f64]| -> f64 {
        a.iter().zip(b).map(|(p, q)| (p - q) * (p - q)).sum()
    };

    let mut best_assign: Vec<usize> = vec![0; n];
    let mut best_inertia = f64::INFINITY;
    let mut rng = Xorshift(0x9E3779B97F4A7C15);

    for _restart in 0..12 {
        // k-means++ init
        let mut cent: Vec<Vec<f64>> = Vec::with_capacity(k);
        let first = (rng.next_f64() * n as f64) as usize % n;
        cent.push(x[first].clone());
        while cent.len() < k {
            let dmin: Vec<f64> = x
                .iter()
                .map(|row| cent.iter().map(|c| dist2(row, c)).fold(f64::INFINITY, f64::min))
                .collect();
            let sum: f64 = dmin.iter().sum();
            let mut t = rng.next_f64() * sum;
            let mut pick = n - 1;
            for (i, &dd) in dmin.iter().enumerate() {
                t -= dd;
                if t <= 0.0 {
                    pick = i;
                    break;
                }
            }
            cent.push(x[pick].clone());
        }

        let mut assign = vec![0usize; n];
        for _it in 0..100 {
            let mut changed = false;
            for (i, row) in x.iter().enumerate() {
                let mut bj = 0;
                let mut bd = f64::INFINITY;
                for (j, c) in cent.iter().enumerate() {
                    let dd = dist2(row, c);
                    if dd < bd {
                        bd = dd;
                        bj = j;
                    }
                }
                if assign[i] != bj {
                    assign[i] = bj;
                    changed = true;
                }
            }
            // recompute centroids
            let mut sums = vec![vec![0.0; d]; k];
            let mut cnt = vec![0usize; k];
            for (i, row) in x.iter().enumerate() {
                let a = assign[i];
                cnt[a] += 1;
                for j in 0..d {
                    sums[a][j] += row[j];
                }
            }
            for c in 0..k {
                if cnt[c] > 0 {
                    for j in 0..d {
                        cent[c][j] = sums[c][j] / cnt[c] as f64;
                    }
                }
            }
            if !changed {
                break;
            }
        }
        let inertia: f64 = x.iter().enumerate().map(|(i, row)| dist2(row, &cent[assign[i]])).sum();
        if inertia < best_inertia {
            best_inertia = inertia;
            best_assign = assign;
        }
    }

    // write assignments
    let of = std::fs::File::create(&outp).expect("create clusters.csv");
    let mut w = BufWriter::new(of);
    writeln!(w, "wallet,cluster").unwrap();
    for (i, wl) in wallets.iter().enumerate() {
        writeln!(w, "{wl},{}", best_assign[i]).unwrap();
    }
    w.flush().unwrap();

    // profile clusters on ORIGINAL feature values
    println!("[cluster] {} wallets, k={}, inertia={:.1}", n, k, best_inertia);
    for c in 0..k {
        let members: Vec<usize> = (0..n).filter(|&i| best_assign[i] == c).collect();
        if members.is_empty() {
            continue;
        }
        let avg = |fi: usize| members.iter().map(|&i| raw[i][fi]).sum::<f64>() / members.len() as f64;
        let med = |fi: usize| {
            let mut v: Vec<f64> = members.iter().map(|&i| raw[i][fi]).collect();
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            v[v.len() / 2]
        };
        println!(
            "\ncluster {c}: {} wallets ({:.0}%)",
            members.len(),
            100.0 * members.len() as f64 / n as f64
        );
        println!(
            "  n(med)={:.0} tokens(med)={:.0} chains(med)={:.0} active_days(med)={:.0}",
            med(0), med(1), med(10), med(2)
        );
        println!(
            "  in_ratio(avg)={:.2} stable_frac(avg)={:.2} primary_share(avg)={:.2} tok_entropy(avg)={:.2} tx/day(med)={:.1}",
            avg(5), avg(6), avg(7), avg(9), med(4)
        );
    }
}
