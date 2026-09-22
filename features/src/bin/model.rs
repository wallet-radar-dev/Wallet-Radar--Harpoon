//! Next-action model on the pilot: does a wallet turn into a net BUYER
//! (accumulator) of non-stable tokens in the *future*, judged only from its
//! *past* behaviour (honest temporal split — features from before a per-wallet
//! cutoff, label from after). Logistic regression, pure std.
//!
//! In: dataset/backfill/transfers.csv
//! Prints: train/test size, baseline vs model accuracy + AUC.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};

fn is_stable(sym: &str) -> bool {
    let s = sym.to_uppercase();
    s.starts_with("USD") || s.ends_with("USD")
        || matches!(s.as_str(), "DAI" | "GHO" | "FRAX" | "TUSD" | "PYUSD" | "USDE" | "USDS" | "RLUSD" | "CRVUSD" | "EURC" | "EURS")
}

fn day_key(ts: &str) -> u32 {
    let b = ts.as_bytes();
    if b.len() < 10 {
        return 0;
    }
    let d = |i: usize| (b[i] as char).to_digit(10).unwrap_or(0);
    let y = d(0) * 1000 + d(1) * 100 + d(2) * 10 + d(3);
    let m = d(5) * 10 + d(6);
    let day = d(8) * 10 + d(9);
    y * 10000 + m * 100 + day
}
fn days(a: u32, b: u32) -> f64 {
    let u = |x: u32| ((x / 10000) as i64) * 365 + (((x / 100) % 100) as i64) * 30 + (x % 100) as i64;
    (u(b) - u(a)).max(0) as f64
}

struct Xorshift(u64);
impl Xorshift {
    fn f(&mut self) -> f64 {
        let mut x = self.0;
        x ^= x << 13; x ^= x >> 7; x ^= x << 17; self.0 = x;
        (x >> 11) as f64 / (1u64 << 53) as f64
    }
}

// packed event: (day, flags) flags: bit0 stable, bit1 out, bit2 in
fn main() {
    let root = std::env::var("APP_ROOT").unwrap_or_else(|_| "/opt/wallet-tracker".into());
    let f = std::fs::File::open(format!("{root}/dataset/backfill/transfers.csv")).expect("open");
    let mut map: HashMap<String, Vec<(u32, u8)>> = HashMap::new();
    for line in BufReader::with_capacity(1 << 20, f).lines() {
        let l = match line { Ok(l) => l, Err(_) => continue };
        let p: Vec<&str> = l.splitn(9, ',').collect();
        if p.len() < 9 || p[0] == "wallet" { continue; }
        let mut fl = 0u8;
        if is_stable(p[5]) { fl |= 1; }
        match p[2] { "o" => fl |= 2, "i" => fl |= 4, _ => {} }
        map.entry(p[0].to_string()).or_default().push((day_key(p[3]), fl));
    }

    // build temporal samples
    let mut feats: Vec<[f64; 6]> = Vec::new();
    let mut labels: Vec<f64> = Vec::new();
    let mut keys: Vec<String> = Vec::new();
    for (w, ev) in map.iter_mut() {
        if ev.len() < 20 { continue; }
        ev.sort_by_key(|e| e.0);
        let cut = ev.len() * 70 / 100;
        let (pre, post) = ev.split_at(cut);
        if pre.is_empty() || post.is_empty() { continue; }
        // label: net non-stable direction in post
        let mut nin = 0i64; let mut nout = 0i64;
        for &(_, fl) in post {
            if fl & 1 == 0 { // non-stable
                if fl & 4 != 0 { nin += 1; }
                if fl & 2 != 0 { nout += 1; }
            }
        }
        if nin + nout == 0 { continue; } // no non-stable action after cutoff
        let label = if nin > nout { 1.0 } else { 0.0 };
        // pre features
        let n = pre.len() as f64;
        let inp = pre.iter().filter(|e| e.1 & 4 != 0).count() as f64;
        let stb = pre.iter().filter(|e| e.1 & 1 != 0).count() as f64;
        let mut daysset: Vec<u32> = pre.iter().map(|e| e.0).collect();
        daysset.dedup();
        let ad = daysset.len() as f64;
        let span = days(pre.first().unwrap().0, pre.last().unwrap().0);
        feats.push([
            (1.0 + n).ln(),
            inp / n,               // in_ratio
            stb / n,               // stable_frac
            (1.0 + ad).ln(),
            (1.0 + n / ad.max(1.0)).ln(), // tx/active-day
            (1.0 + span).ln(),
        ]);
        labels.push(label);
        keys.push(w.clone());
    }
    let m = feats.len();
    let d = 6;

    // standardize
    let mut mean = [0.0; 6]; let mut sd = [0.0; 6];
    for r in &feats { for j in 0..d { mean[j] += r[j]; } }
    for j in 0..d { mean[j] /= m as f64; }
    for r in &feats { for j in 0..d { sd[j] += (r[j]-mean[j]).powi(2); } }
    for j in 0..d { sd[j] = (sd[j]/m as f64).sqrt().max(1e-9); }
    let z: Vec<[f64;6]> = feats.iter().map(|r| { let mut o=[0.0;6]; for j in 0..d { o[j]=(r[j]-mean[j])/sd[j]; } o }).collect();

    // train/test split by stable hash of wallet
    let hsh = |s:&str| -> u64 { let mut h=1469598103934665603u64; for b in s.bytes(){ h^=b as u64; h=h.wrapping_mul(1099511628211);} h };
    let test: Vec<bool> = keys.iter().map(|k| hsh(k)%5==0).collect(); // ~20%

    // logistic regression (GD) on train
    let mut w = [0.0f64; 6]; let mut b = 0.0;
    let lr = 0.1;
    for _ in 0..2000 {
        let mut gw=[0.0;6]; let mut gb=0.0; let mut nt=0.0;
        for i in 0..m {
            if test[i] { continue; }
            nt+=1.0;
            let zsum: f64 = (0..d).map(|j| w[j]*z[i][j]).sum::<f64>() + b;
            let p = 1.0/(1.0+(-zsum).exp());
            let e = p - labels[i];
            for j in 0..d { gw[j]+=e*z[i][j]; }
            gb+=e;
        }
        if nt==0.0 { break; }
        for j in 0..d { w[j]-=lr*gw[j]/nt; }
        b-=lr*gb/nt;
    }

    // evaluate on test
    let mut probs=Vec::new(); let mut ys=Vec::new();
    let mut correct=0.0; let mut ntest=0.0; let mut pos=0.0;
    for i in 0..m {
        if !test[i] { continue; }
        ntest+=1.0; pos+=labels[i];
        let zsum: f64 = (0..d).map(|j| w[j]*z[i][j]).sum::<f64>() + b;
        let p = 1.0/(1.0+(-zsum).exp());
        if (p>=0.5)==(labels[i]>0.5) { correct+=1.0; }
        probs.push(p); ys.push(labels[i]);
    }
    // AUC (rank-based)
    let mut idx: Vec<usize> = (0..probs.len()).collect();
    idx.sort_by(|&a,&b| probs[a].partial_cmp(&probs[b]).unwrap());
    let mut rank_sum=0.0; let mut np=0.0; let mut nn=0.0;
    for (r,&i) in idx.iter().enumerate() { if ys[i]>0.5 { rank_sum += (r+1) as f64; np+=1.0; } else { nn+=1.0; } }
    let auc = if np>0.0 && nn>0.0 { (rank_sum - np*(np+1.0)/2.0)/(np*nn) } else { f64::NAN };
    let base = (pos/ntest).max(1.0-pos/ntest); // majority-class baseline
    let train_n = m as f64 - ntest;

    let _rng = Xorshift(1); // (reserved)
    println!("[next-action] samples={m} (train={train_n:.0} test={ntest:.0}), post-label pos-rate(test)={:.2}", pos/ntest);
    println!("  baseline (majority) acc = {:.3}", base);
    println!("  model    test        acc = {:.3}", correct/ntest);
    println!("  model    test        AUC = {:.3}", auc);
    println!("  weights (ln n, in_ratio, stable_frac, ln active_days, ln tx/day, ln span):");
    println!("    {:?}", w.iter().map(|x| (x*1000.0).round()/1000.0).collect::<Vec<_>>());
}
