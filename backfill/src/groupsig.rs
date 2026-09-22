//! groupsig — СИГНАЛИ ПО ВСІХ ТОКЕНАХ ГРУПИ КАПІТАЛУ, з емісією на ВСЮ групу.
//!
//! Для кожного токена, який тримає група:
//!   • ХОЛДИНГ ГРУПИ — сума по ВСІХ членах. Для значущих позицій (≥MIN_USD) береться РЕАЛЬНИЙ
//!     баланс через Etherscan; дрібні лишаються flow-net оцінкою і позначені як оцінка.
//!     (Причина: перевірка показала, що flow-net завищує ~×8, а третини позицій не існує взагалі.)
//!   • ЕМІСІЯ — холдинг / total supply токена (supply тягнеться з Etherscan, не з локального файлу,
//!     бо в token-supply.json є лише 167 із 574 цінованих).
//!   • ЛІКВІДНІСТЬ — скільки насправді можна вийняти: min(холдинг, LIQ_FRAC×пули) і за скільки днів
//!     обороту позиція виходить.
//!   • ФАЗА — нетто-потік групи по токену за останні PHASE_D днів: набирає / роздає / тримає.
//! Сигнал = комбінація: контроль емісії × фаза × можливість вийти.
//!
//! Вихід: dataset/backfill/group-token-signals.json + оновлює assets у dataset/capital-groups.json.
//! Args: ROOT [GROUP]. Env: MIN_USD(3000) LIQ_FRAC(0.10) PHASE_D(90) CONC(8) WRITE(1) TG(0)
use backfill::chain::ChainSource;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::sync::{Arc, Mutex};
use tokio::sync::Semaphore;

fn day_from_iso(ts: &str) -> i64 {
    if ts.len() < 10 { return 0; }
    let (y, m, d): (i64, i64, i64) = (ts[0..4].parse().unwrap_or(0), ts[5..7].parse().unwrap_or(0), ts[8..10].parse().unwrap_or(0));
    if y < 2015 { return 0; }
    let yy = if m <= 2 { y - 1 } else { y };
    let era = (if yy >= 0 { yy } else { yy - 399 }) / 400;
    let yoe = yy - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    era * 146097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719468
}
// Доступ до ланцюга — через backfill::chain: ротація ключів, ловля NOTOK у ТІЛІ,
// відступи й пошук ключів (оточення → deploy/backfill.env → .env) живуть там.
// Власна es() шукала ключі лише в .env, де їх немає з 19.08 — бін падав на assert.

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let group = std::env::args().nth(2).unwrap_or_else(|| "NET".into());
    let min_usd: f64 = std::env::var("MIN_USD").ok().and_then(|s| s.parse().ok()).unwrap_or(3000.0);
    let liq_frac: f64 = std::env::var("LIQ_FRAC").ok().and_then(|s| s.parse().ok()).unwrap_or(0.10);
    let phase_d: i64 = std::env::var("PHASE_D").ok().and_then(|s| s.parse().ok()).unwrap_or(90);
    let conc: usize = std::env::var("CONC").ok().and_then(|s| s.parse().ok()).unwrap_or(8);
    // У скільки разів оцінка позиції може перевищити реальний пул, перш ніж її визнають
    // непридатною. Грубо навмисно: ловимо абсурд, а не важкі чесні позиції.
    let pool_sanity: f64 = std::env::var("POOL_SANITY").ok().and_then(|s| s.parse().ok()).unwrap_or(1_000_000.0);
    let chain_filter = std::env::var("CHAIN_FILTER").unwrap_or_else(|_| "eth".into());
    let write = std::env::var("WRITE").map(|v| v != "0").unwrap_or(true);
    let bf = format!("{root}/dataset/backfill");
    let src = std::sync::Arc::new(ChainSource::from_env(&root, 1));

    // члени групи
    let memb: Value = serde_json::from_reader(BufReader::new(File::open(format!("{bf}/group-membership.json")).expect("membership"))).unwrap();
    let mut members: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (w, x) in memb["wallets"].as_object().cloned().unwrap_or_default() {
        if x["group"].as_str() == Some(group.as_str()) { members.insert(w.to_lowercase()); }
    }
    eprintln!("[groupsig] група {group}: членів {}", members.len());

    // ціни / ліквідність / десяткові
    let tp: Value = serde_json::from_reader(BufReader::new(File::open(format!("{bf}/token-prices.json")).expect("token-prices"))).unwrap();
    let mut px: HashMap<String, (f64, String)> = tp["tokens"].as_object().map(|m| m.iter()
        .filter_map(|(k, v)| v["usd"].as_f64().map(|u| (k.to_lowercase(), (u, v["symbol"].as_str().unwrap_or("").to_string())))).collect()).unwrap_or_default();
    if let Ok(oc) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(File::open(format!("{bf}/onchain-prices.json")).unwrap())) {
        for (k, v) in &oc { let k = k.to_lowercase();
            if px.contains_key(&k) { continue; }
            if let Some(u) = v["usd"].as_f64() { if u > 0.0 && u < 10_000.0 { px.insert(k, (u, String::new())); } } }
    }
    let liq: HashMap<String, (f64, f64, String)> = serde_json::from_reader::<_, Value>(BufReader::new(File::open(format!("{bf}/token-liquidity.json")).unwrap())).ok()
        .and_then(|v| v["tokens"].as_object().map(|m| m.iter().map(|(k, t)| (k.to_lowercase(),
            (t["reserveUsd"].as_f64().unwrap_or(0.0), t["vol24"].as_f64().unwrap_or(0.0), t["symbol"].as_str().unwrap_or("").to_string()))).collect())).unwrap_or_default();
    let mut dec: HashMap<String, i32> = HashMap::new();
    let mut sym_of: HashMap<String, String> = HashMap::new();
    if let Ok(sv) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(File::open(format!("{root}/dataset/token-supply.json")).unwrap())) {
        for (c, v) in &sv { let c = c.to_lowercase();
            dec.insert(c.clone(), v["decimals"].as_u64().unwrap_or(18) as i32);
            sym_of.insert(c, v["symbol"].as_str().unwrap_or("").to_string()); }
    }

    // прохід по transfers: net на (член,токен) + нетто групи за останні PHASE_D днів
    let today = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() / 86400) as i64;
    let mut pair: HashMap<(String, String), f64> = HashMap::new();
    let mut recent: HashMap<String, f64> = HashMap::new();
    for line in BufReader::with_capacity(1 << 20, File::open(format!("{bf}/transfers.csv")).expect("transfers")).lines().map_while(Result::ok) {
        let mut it = line.splitn(6, ',');
        let w = it.next().unwrap_or(""); let ch = it.next().unwrap_or(""); let d = it.next().unwrap_or(""); let ts = it.next().unwrap_or(""); let tok = it.next().unwrap_or("");
        if w == "wallet" || (d != "i" && d != "o") { continue; }
        // ЛИШЕ mainnet: ціни, ліквідність і supply ми тягнемо для Ethereum, а в transfers.csv є й L2.
        // Без цього фільтра топ-позицією ставав ARB (адреса контракту Arbitrum) на $52М — величина,
        // до якої mainnet-довідники просто не застосовні.
        if ch != chain_filter { continue; }
        let wl = w.to_lowercase(); if !members.contains(&wl) { continue; }
        let tl = tok.to_lowercase(); if !px.contains_key(&tl) { continue; }
        let amt: f64 = line.rsplitn(4, ',').nth(1).unwrap_or("0").parse().unwrap_or(0.0);
        if amt <= 0.0 { continue; }
        let sgn = if d == "i" { amt } else { -amt };
        *pair.entry((wl, tl.clone())).or_insert(0.0) += sgn;
        if today - day_from_iso(ts) <= phase_d { *recent.entry(tl).or_insert(0.0) += sgn; }
    }
    // холдинг групи за оцінкою + пари, які варто перевірити
    let mut est: HashMap<String, f64> = HashMap::new();
    let mut to_check: Vec<(String, String, f64)> = Vec::new();
    for ((w, t), v) in &pair {
        if *v <= 0.0 { continue; }
        *est.entry(t.clone()).or_insert(0.0) += v;
        if v * px[t].0 >= min_usd { to_check.push((w.clone(), t.clone(), *v)); }
    }
    eprintln!("[groupsig] токенів у групі: {} · значущих пар до перевірки: {}", est.len(), to_check.len());

    // РЕАЛЬНІ баланси значущих пар + supply токенів
    let sem = Arc::new(Semaphore::new(conc));
    let real: Arc<Mutex<HashMap<(String, String), f64>>> = Arc::new(Mutex::new(HashMap::new()));
    let mut hs = Vec::new();
    for (w, t, _) in to_check.clone() {
        let (src, sem, real, dec) = (src.clone(), sem.clone(), real.clone(), dec.clone());
        hs.push(tokio::spawn(async move {
            let _p = sem.acquire().await.unwrap();
            // None = не спитали (ліміт). Нуль сюди не пишемо: «не знаємо балансу» і
            // «балансу немає» дали б однакову частку емісії, а це різні твердження.
            if let Some(raw) = src.token_balance(&t, &w).await {
                let d = dec.get(&t).copied().unwrap_or(18);
                real.lock().unwrap().insert((w, t), raw as f64 / 10f64.powi(d));
            }
        }));
    }
    for h in hs { let _ = h.await; }
    let real = real.lock().unwrap().clone();

    let toks: Vec<String> = est.keys().cloned().collect();
    let supply: Arc<Mutex<HashMap<String, f64>>> = Arc::new(Mutex::new(HashMap::new()));
    let mut hs2 = Vec::new();
    for t in toks.clone() {
        let (src, sem, supply, dec) = (src.clone(), sem.clone(), supply.clone(), dec.clone());
        hs2.push(tokio::spawn(async move {
            let _p = sem.acquire().await.unwrap();
            if let Some(raw) = src.token_supply(&t).await {
                let d = dec.get(&t).copied().unwrap_or(18);
                let v = raw as f64 / 10f64.powi(d);
                if v > 0.0 { supply.lock().unwrap().insert(t, v); }
            }
        }));
    }
    for h in hs2 { let _ = h.await; }
    let supply = supply.lock().unwrap().clone();
    eprintln!("[groupsig] перевірено балансів: {} · отримано supply: {}/{}", real.len(), supply.len(), toks.len());

    // зведення по токенах
    let mut rows: Vec<Value> = Vec::new();
    for t in &toks {
        let (price, psym) = px[t].clone();
        // холдинг: реальні там, де перевіряли; оцінка — де ні
        let (mut hold, mut verified, mut est_part) = (0.0f64, 0.0f64, 0.0f64);
        for ((w, tt), v) in &pair {
            if tt != t || *v <= 0.0 { continue; }
            match real.get(&(w.clone(), tt.clone())) { Some(r) => { hold += r; verified += r * price; } None => { hold += v; est_part += v * price; } }
        }
        let usd = hold * price;
        if usd < 500.0 { continue; }
        let (res, vol, lsym) = liq.get(t).cloned().unwrap_or((0.0, 0.0, String::new()));
        let sym = [lsym, psym, sym_of.get(t).cloned().unwrap_or_default()].into_iter().find(|s| !s.is_empty()).unwrap_or_else(|| t[0..8].to_string());
        // Фаза 0.2: частка емісії + реалізовне — зі СЛОВНИКА (той самий вираз) → байт-паритет.
        let emission = supply.get(t).map(|s| wr_dict::emission_share(hold, *s));
        let exit_usd = wr_dict::realizable_usd(usd, res, liq_frac);
        let days_vol = if vol > 0.0 { usd / vol } else { -1.0 };
        let net90 = recent.get(t).copied().unwrap_or(0.0) * price;
        let phase = if net90 > 0.05 * usd { "НАБИРАЄ" } else if net90 < -0.05 * usd { "РОЗДАЄ" } else { "ТРИМАЄ" };
        // сигнал: контроль × фаза × чи можна вийти
        let ctrl = emission.unwrap_or(0.0);
        // ВАГА ПОЗИЦІЇ = скільки днів обороту вона становить. Частка емісії виявилась нулем
        // (0–1.2%) і як міра значущості не працює; днів обороту показує реальний тиск на ринок:
        // $200 тис. у монеті з обігом $8 тис./добу важать більше, ніж $2 млн у ліквідному ONDO.
        let heavy = days_vol >= 30.0;                 // позиція «важка» для ринку
        let signal = if phase == "РОЗДАЄ" && heavy { "🔴 РОЗДАЄ ВАЖКУ ПОЗИЦІЮ — тиск на ринок" }
            else if phase == "НАБИРАЄ" && heavy { "🟢 НАБИРАЄ ВАЖКУ ПОЗИЦІЮ" }
            else if heavy { "🔒 ВАЖКА ПОЗИЦІЯ БЕЗ РУХУ — вийти без просідання не вийде" }
            else if phase == "РОЗДАЄ" { "🟠 роздає (ринок перетравить)" }
            else if ctrl >= 0.10 { "ℹ️ помітна частка емісії" }
            else { "⚪ без сигналу" };
        // ПОЗИЦІЯ, ЯКУ НЕМА ЧИМ ПЕРЕВІРИТИ, не йде в гроші групи.
        //
        // Що сталося 14.09.2026: NET-WEAK показала холдинг $90 361 704 633 545 — дев'яносто
        // трильйонів. Винен один рядок: ZENT (0x3e985250…), 3.05e16 одиниць, ціна $0.00296 з
        // onchain-prices, де по ньому ВСЬОГО 15 свопів, а реальний пул — $1398. Тридцять
        // квадрильйонів одиниць сміттєвого аірдропу помножились на ціну з мертвого ринку.
        // Баланс при цьому справжній, підтверджений у ланцюзі: бреше не він, а оцінка.
        //
        // Два незалежні приводи не довіряти оцінці:
        //   * НЕВІДОМИЙ SUPPLY — нема емісії, а емісія в цьому проєкті і є лінійкою
        //     (див. wr-dict::emission_share). Без неї 3e16 одиниць не відрізнити від позиції;
        //   * ОЦІНКА ПРОТИ ПУЛУ — вартість більша за реальний пул у POOL_SANITY разів
        //     (типово мільйон). Навіть найважча чесна позиція не буває в мільйон разів
        //     більшою за ринок, у якому живе; поріг навмисно грубий, щоб не зачепити
        //     справжні «важкі позиції», заради яких цей звіт і робиться.
        //
        // Рядок лишається у виводі — володіння реальне, і ховати його не можна. Але в
        // підсумок він не входить і позначений прапорцем, щоб жоден споживач не склав
        // трильйони мовчки.
        let no_supply = supply.get(t).is_none();
        let vs_pool = res > 0.0 && usd > pool_sanity * res;
        let unpriceable = no_supply || vs_pool;
        if unpriceable {
            eprintln!("[groupsig] ⚠️ {sym} ({t}): оцінка ${usd:.0} НЕ зарахована — {}",
                if no_supply { "невідома емісія (supply не отримано)".to_string() }
                else { format!("це в {:.0}× більше за реальний пул ${res:.0}", usd / res.max(1.0)) });
        }
        rows.push(json!({"symbol": sym, "token": t, "holdings": hold, "usd": usd,
            "usdVerified": verified, "usdEstimated": est_part,
            "emissionPct": emission.map(|e| e * 100.0), "supply": supply.get(t),
            "reserveUsd": res, "vol24": vol, "exitableUsd": exit_usd, "daysOfVolume": days_vol,
            "net90dUsd": net90, "phase": phase, "signal": signal,
            "unpriceable": unpriceable}));
    }
    rows.sort_by(|a, b| b["usd"].as_f64().unwrap_or(0.0).partial_cmp(&a["usd"].as_f64().unwrap_or(0.0)).unwrap());

    eprintln!("[groupsig] ===== СИГНАЛИ ПО ТОКЕНАХ ГРУПИ {group} =====");
    eprintln!("{:<10} {:>13} {:>9} {:>13} {:>9} {:<10} {}", "токен", "холдинг $", "емісія%", "вийти можна$", "днів обіг", "фаза", "сигнал");
    for r in rows.iter().take(25) {
        eprintln!("{:<10} {:>13.0} {:>9} {:>13.0} {:>9} {:<10} {}",
            r["symbol"].as_str().unwrap_or("?"), r["usd"].as_f64().unwrap_or(0.0),
            r["emissionPct"].as_f64().map(|e| format!("{e:.1}")).unwrap_or_else(|| "?".into()),
            r["exitableUsd"].as_f64().unwrap_or(0.0),
            { let d = r["daysOfVolume"].as_f64().unwrap_or(-1.0); if d < 0.0 { "—".to_string() } else { format!("{d:.0}") } },
            r["phase"].as_str().unwrap_or(""), r["signal"].as_str().unwrap_or(""));
    }
    // Підсумок рахується ЛИШЕ по тому, що піддається перевірці (див. `unpriceable` вище).
    let priced = |r: &Value| !r["unpriceable"].as_bool().unwrap_or(false);
    let (tot, tot_exit): (f64, f64) = (
        rows.iter().filter(|r| priced(r)).map(|r| r["usd"].as_f64().unwrap_or(0.0)).sum(),
        rows.iter().filter(|r| priced(r)).map(|r| r["exitableUsd"].as_f64().unwrap_or(0.0)).sum());
    let skipped = rows.iter().filter(|r| !priced(r)).count();
    eprintln!("[groupsig] токенів {} · холдинг ${:.0} · реально можна вийняти ${:.0} ({:.1}%)",
        rows.len(), tot, tot_exit, if tot > 0.0 { 100.0 * tot_exit / tot } else { 0.0 });
    if skipped > 0 {
        eprintln!("[groupsig] поза підсумком: {skipped} позицій без перевірної оцінки (у tokens[] вони є, з прапорцем unpriceable)");
    }

    fs::write(format!("{bf}/group-token-signals.json"), serde_json::to_string_pretty(&json!({
        "group": group, "members": members.len(), "minUsd": min_usd, "liqFrac": liq_frac, "phaseDays": phase_d,
        "totalUsd": tot, "exitableUsd": tot_exit, "tokens": rows})).unwrap()).ok();

    if write {
        let cg = format!("{root}/dataset/capital-groups.json");
        if let Ok(mut v) = serde_json::from_reader::<_, Value>(BufReader::new(File::open(&cg).unwrap())) {
            fs::copy(&cg, format!("{cg}.bak-sig")).ok();
            let by: HashMap<String, &Value> = rows.iter().map(|r| (r["symbol"].as_str().unwrap_or("").to_string(), r)).collect();
            if let Some(gs) = v["groups"].as_array_mut() {
                for g in gs.iter_mut() {
                    if g["id"].as_str() != Some(group.as_str()) { continue; }
                    g["tokenSignals"] = json!(rows);
                    if let Some(as_) = g["assets"].as_array_mut() {
                        for a in as_.iter_mut() {
                            if let Some(src) = by.get(a["symbol"].as_str().unwrap_or("")) {
                                a["signal"] = src["signal"].clone();
                                a["emissionPctGroup"] = src["emissionPct"].clone();
                                a["exitableUsd"] = src["exitableUsd"].clone();
                            }
                        }
                    }
                }
            }
            fs::write(&cg, serde_json::to_string_pretty(&v).unwrap()).ok();
            eprintln!("[groupsig] capital-groups.json оновлено (бекап .bak-sig)");
        }
    }
    eprintln!("[groupsig] збережено group-token-signals.json");
}
