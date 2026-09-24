//! sigcheck — дозрівання журналу сигналів і чесний підсумок: чи вони взагалі працюють.
//!
//! Журнал (`wr_store::journal`) накопичує твердження: сплеск у стакані, роздача, зміна
//! частки емісії — кожне з ціною на момент події. Цей бін робить другу половину роботи:
//! повертається до старих записів, коли минув горизонт, і рахує, що сталося з ціною.
//!
//! ЧОМУ НАДЛИШОК, А НЕ ДОХІДНІСТЬ. «Після сигналу ціна зросла на 4%» не означає нічого:
//! у бичачий тиждень зростає майже все. Єдине число, за яким можна судити, — різниця
//! між активом і ринком за те саме вікно. Тому кожен результат містить дохідність
//! активу, дохідність базового активу (ETH) і різницю; присуд виноситься лише за нею.
//!
//! ЧОМУ ЦІНИ З DefiLlama. Он-чейн ціни в цьому репозиторії доведено брешуть (COMP ×2350,
//! див. `tokprices.rs`). Обидва кінці вікна беруться з одного джерела й за КОНТРАКТОМ,
//! а не за тикером: тикерів-двійників у наших даних вистачає.
//!
//! ЧОМУ МЕРЕЖЕВИЙ ЗБІЙ НЕ ЗАПИСУЄТЬСЯ. Якщо API не відповів, сигнал лишається
//! недозрілим і чекає наступного прогону. Записати «результату немає» через хвилинну
//! недоступність означало б назавжди викинути спостереження з вибірки — саме той тихий
//! перекіс, від якого журнал і рятує. Остаточна відмова пишеться лише тоді, коли
//! джерело ВІДПОВІЛО, ціни немає, і від дозрівання минуло понад GIVEUP_D днів.
//!
//! Args: ROOT [mature|report|both]
//! Env: HORIZONS(1,7,30,90) MIN_N(30) BENCH(coingecko:ethereum) TOL_H(6) GIVEUP_D(5)
//!      BATCH(20) SLEEP_MS(300) MAX_POINTS(600) DRY(0)

use serde_json::Value;
use std::collections::{HashMap, HashSet};
use wr_store::journal::{Journal, Outcome, Signal, SCHEMA};

fn env_s(k: &str, d: &str) -> String { std::env::var(k).unwrap_or_else(|_| d.to_string()) }
fn env_u(k: &str, d: u64) -> u64 { std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d) }
fn env_f(k: &str, d: f64) -> f64 { std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d) }

fn now_s() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn ymd(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0).map(|d| d.format("%Y-%m-%d").to_string()).unwrap_or_default()
}

/// Креденшли Telegram лежать у `<ROOT>/.env` — стала домовленість бінів цього репозиторію.
fn envv(root: &str, k: &str) -> String {
    std::fs::read_to_string(format!("{root}/.env")).unwrap_or_default().lines()
        .find(|l| l.starts_with(&format!("{k}=")))
        .map(|l| l.splitn(2, '=').nth(1).unwrap_or("").trim().to_string())
        .unwrap_or_default()
}

async fn tg(client: &reqwest::Client, token: &str, chat: &str, text: &str) -> bool {
    if token.is_empty() || chat.is_empty() { eprintln!("[sigcheck] нема TELEGRAM креденшлів — звіт лишиться в консолі"); return false }
    matches!(client.post(format!("https://api.telegram.org/bot{token}/sendMessage"))
        .form(&[("chat_id", chat), ("text", text), ("parse_mode", "HTML"), ("disable_web_page_preview", "true")])
        .send().await, Ok(r) if r.status().is_success())
}

fn median(v: &mut Vec<f64>) -> f64 {
    if v.is_empty() { return 0.0 }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = v.len();
    if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) / 2.0 }
}

/// Ціна монети в конкретний момент минулого.
///
/// `searchWidth` задає, наскільки далеко джерелу дозволено шукати найближчу точку;
/// повернутий час ми ПЕРЕВІРЯЄМО самі й відкидаємо все, що далі за допуск. Інакше для
/// неліквідного токена прилетіла б ціна тижневої давнини й виглядала як сьогоднішня.
async fn fetch_points(
    client: &reqwest::Client, ts: i64, coins: &[String], tol_s: i64,
) -> Result<HashMap<String, f64>, String> {
    let url = format!(
        "https://coins.llama.fi/prices/historical/{ts}/{}?searchWidth={}h",
        coins.join(","), (tol_s / 3600).max(1)
    );
    let r = client.get(&url).send().await.map_err(|e| format!("запит: {e}"))?;
    if !r.status().is_success() { return Err(format!("код {}", r.status())) }
    let v: Value = r.json().await.map_err(|e| format!("розбір: {e}"))?;
    let mut out = HashMap::new();
    let Some(obj) = v["coins"].as_object() else { return Ok(out) };
    for (k, c) in obj {
        let (Some(p), Some(t)) = (c["price"].as_f64(), c["timestamp"].as_i64()) else { continue };
        if p <= 0.0 { continue }
        if (t - ts).abs() > tol_s { continue }   // задалеко — мовчати краще, ніж підставити чуже
        out.insert(k.to_lowercase(), p);
    }
    Ok(out)
}

fn coin_of(s: &Signal) -> Option<String> {
    if s.contract.is_empty() { return None }
    let chain = if s.chain.is_empty() { "ethereum" } else { &s.chain };
    Some(format!("{chain}:{}", s.contract.to_lowercase()))
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let mode = std::env::args().nth(2).unwrap_or_else(|| "both".into());
    let horizons: Vec<u32> = env_s("HORIZONS", "1,7,30,90").split(',')
        .filter_map(|s| s.trim().parse().ok()).filter(|h| *h > 0).collect();
    let min_n = env_u("MIN_N", 30) as usize;
    let bench = env_s("BENCH", "coingecko:ethereum").to_lowercase();
    let tol_s = (env_f("TOL_H", 6.0) * 3600.0) as i64;
    let giveup_s = env_u("GIVEUP_D", 5) as i64 * 86_400;
    let batch = env_u("BATCH", 20) as usize;
    let sleep_ms = env_u("SLEEP_MS", 300);
    let max_points = env_u("MAX_POINTS", 600) as usize;
    let dry = env_u("DRY", 0) == 1;

    let dir = wr_store::journal::journal_dir_for(&root);
    let mut j = match Journal::open_at(&dir) {
        Ok(j) => j,
        Err(e) => { eprintln!("[sigcheck] журнал не відкрився: {e}"); std::process::exit(2) }
    };
    let signals = match j.signals() {
        Ok(v) => v,
        Err(e) => { eprintln!("[sigcheck] журнал не читається: {e}"); std::process::exit(2) }
    };
    eprintln!("[sigcheck] журнал: {} · сигналів {}", dir.display(), signals.len());
    if signals.is_empty() {
        // Порожній журнал — не привід рапортувати успіх: або джерела ще не писали,
        // або пишуть не туди. Мовчазний нуль тут виглядав би як «усе гаразд».
        eprintln!("[sigcheck] у журналі порожньо — перевір, що джерела сигналів бачать ту саму теку");
        std::process::exit(1);
    }

    let now = now_s();
    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(40)).build().unwrap();

    // ───────────────────────── дозрівання ─────────────────────────
    let mut added = 0usize;
    let mut skipped_no_contract = 0usize;
    let mut left_pending = 0usize;

    if mode != "report" {
        // Що треба порахувати: пара (сигнал, горизонт), у якої горизонт уже минув,
        // а результату ще немає.
        let mut due: Vec<(&Signal, u32, i64)> = Vec::new();
        for s in &signals {
            for &h in &horizons {
                let mature_at = s.ts + h as i64 * 86_400;
                if mature_at > now || j.has_outcome(&s.id, h) { continue }
                due.push((s, h, mature_at));
            }
        }
        // спершу найстаріші: якщо впремося в MAX_POINTS, доростає те, що чекає найдовше
        due.sort_by_key(|(_, _, m)| *m);
        eprintln!("[sigcheck] до дозрівання: {} пар (сигнал×горизонт)", due.len());

        // Точки цін, які доведеться дістати. Групуємо за МОМЕНТОМ: один запит віддає
        // ціни багатьох монет на один час, і сигнали одного вікна діляться запитом.
        let mut want: HashMap<i64, HashSet<String>> = HashMap::new();
        let mut usable: Vec<(&Signal, u32, i64)> = Vec::new();
        for (s, h, mature_at) in due {
            let Some(coin) = coin_of(s) else {
                // Актив без контракту неперевірний. Відмову пишемо лише після GIVEUP_D:
                // контракт може з'явитись у списку спостереження пізніше.
                if now - mature_at > giveup_s {
                    let o = Outcome::failed(&s.id, h, now, "no-contract",
                        &format!("{}: у записі немає контракту — ціну нема за чим шукати", s.sym));
                    if j.append_outcome(&o).unwrap_or(false) { skipped_no_contract += 1 }
                } else { left_pending += 1 }
                continue
            };
            want.entry(s.ts).or_default().insert(coin.clone());
            want.entry(mature_at).or_default().insert(coin);
            want.entry(s.ts).or_default().insert(bench.clone());
            want.entry(mature_at).or_default().insert(bench.clone());
            usable.push((s, h, mature_at));
        }

        // Ліміт на прохід: квота джерела спільна з рештою бінів, тож беремо порціями.
        let mut points: Vec<(i64, Vec<String>)> = want.into_iter()
            .map(|(ts, set)| (ts, set.into_iter().collect::<Vec<_>>())).collect();
        points.sort_by_key(|(ts, _)| *ts);
        let mut budget = max_points;
        let mut prices: HashMap<(i64, String), f64> = HashMap::new();
        let mut net_fail = 0usize;
        'outer: for (ts, coins) in points {
            for chunk in coins.chunks(batch) {
                if budget == 0 { eprintln!("[sigcheck] вичерпано ліміт точок на прохід — решта дозріє наступного разу"); break 'outer }
                budget = budget.saturating_sub(chunk.len());
                match fetch_points(&client, ts, chunk, tol_s).await {
                    Ok(m) => for (k, p) in m { prices.insert((ts, k), p); },
                    Err(e) => { net_fail += 1; eprintln!("[sigcheck] {ts}: {e}") }
                }
                tokio::time::sleep(std::time::Duration::from_millis(sleep_ms)).await;
            }
        }
        if net_fail > 0 { eprintln!("[sigcheck] невдалих запитів: {net_fail} — ці пари лишаються недозрілими, не відмовленими") }

        for (s, h, mature_at) in usable {
            let Some(coin) = coin_of(s) else { continue };
            let p0 = prices.get(&(s.ts, coin.clone())).copied();
            let p1 = prices.get(&(mature_at, coin.clone())).copied();
            let b0 = prices.get(&(s.ts, bench.clone())).copied();
            let b1 = prices.get(&(mature_at, bench.clone())).copied();

            match (p0, p1, b0, b1) {
                (Some(p0), Some(p1), Some(b0), Some(b1)) if p0 > 0.0 && b0 > 0.0 => {
                    let ret = 100.0 * (p1 - p0) / p0;
                    let bret = 100.0 * (b1 - b0) / b0;
                    let o = Outcome {
                        v: SCHEMA, id: s.id.clone(), horizon_d: h, checked_at: now,
                        price0: Some(p0), price1: Some(p1),
                        ret_pct: Some(ret), bench_ret_pct: Some(bret),
                        excess_pct: Some(ret - bret),
                        status: "ok".into(), reason: String::new(),
                    };
                    if j.append_outcome(&o).unwrap_or(false) { added += 1 }
                }
                _ => {
                    if now - mature_at > giveup_s {
                        let what = match (p0.is_some(), p1.is_some(), b0.is_some() && b1.is_some()) {
                            (false, _, _) => "немає ціни на момент сигналу",
                            (_, false, _) => "немає ціни на кінець горизонту",
                            (_, _, false) => "немає ціни базового активу",
                            _ => "ціни нульові",
                        };
                        let o = Outcome::failed(&s.id, h, now, "no-price",
                            &format!("{} ({coin}): {what}", s.sym));
                        if j.append_outcome(&o).unwrap_or(false) { added += 1 }
                    } else { left_pending += 1 }
                }
            }
        }
        eprintln!("[sigcheck] записано результатів: {added} (без контракту {skipped_no_contract}) · лишилось чекати {left_pending}");
    }

    // ───────────────────────── звіт ─────────────────────────
    if mode == "mature" { return }

    let outcomes = match j.outcomes() { Ok(v) => v, Err(e) => { eprintln!("[sigcheck] результати не читаються: {e}"); std::process::exit(2) } };
    let by_id: HashMap<&str, &Signal> = signals.iter().map(|s| (s.id.as_str(), s)).collect();

    // Нульова база з ретро-реплею: що дав би ВИПАДКОВИЙ момент входу в той самий актив
    // на тому самому горизонті. Без неї «після сигналу +9% до ринку» може означати лише
    // те, що актив увесь період обганяв ринок, і будь-яка дата дала б те саме.
    let null_base: Value = std::fs::read_to_string(dir.join("retro-null.json")).ok()
        .and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null);

    // (джерело, вид, горизонт) -> надлишки
    let mut cells: HashMap<(String, String, u32), Vec<f64>> = HashMap::new();
    // той самий ключ -> скільки сигналів дав кожен актив (показує, чи вибірка не з одного токена)
    let mut cell_syms: HashMap<(String, String, u32), HashMap<String, usize>> = HashMap::new();
    let mut cell_ex: HashMap<(String, String, u32), Vec<f64>> = HashMap::new();
    // той самий зріз, але за часткою емісії: наша лінійка, а не долари
    let mut by_emis: HashMap<(String, String, u32, &'static str), Vec<f64>> = HashMap::new();
    let mut unresolved = 0usize;
    let mut no_null = 0usize;
    // Тікер базового активу: його сигнали виключаються з підсумку. Надлишок активу
    // проти самого себе тотожно нульовий, тож кожен такий запис тягнув би статистику
    // до нуля й вдавав, що сигнал нічого не дає. ETH є у вотчлісті як мажор мережі,
    // і в першому ретро-прогоні дав 994 таких порожніх спостереження.
    let bench_sym = env_s("BENCH_SYM", "ETH").to_uppercase();
    let mut self_bench = 0usize;

    for o in &outcomes {
        let Some(s) = by_id.get(o.id.as_str()) else { continue };
        if s.sym.to_uppercase() == bench_sym { self_bench += 1; continue }
        if o.status != "ok" { unresolved += 1; continue }
        let Some(hit) = o.hit(&s.kind) else { continue };
        let signed = if hit { 1.0 } else { 0.0 };
        let key = (s.source.clone(), s.kind.clone(), o.horizon_d);
        cells.entry(key.clone()).or_default().push(signed);
        *cell_syms.entry(key.clone()).or_default().entry(s.sym.clone()).or_insert(0) += 1;
        cell_ex.entry(key.clone()).or_default().push(o.excess_pct.unwrap_or(0.0));

        // Для зрізів беремо НАДЛИШОК ПОНАД ВИПАДКОВИЙ ВХІД у цей самий актив: інакше
        // зріз показує не силу сигналу, а те, які активи в нього потрапили. Актив, що
        // весь період обганяв ринок, інакше давав би «сильний сигнал» у кожному кошику.
        let tf = s.source.strip_prefix("retro-").unwrap_or("");
        let null_med = null_base.pointer(&format!("/by_tf/{tf}/{}/{}", s.sym, o.horizon_d))
            .and_then(|v| v["median_excess"].as_f64());
        // надлишок зберігаємо окремо від влучань — медіана надлишку й частка влучань
        // кажуть різне: можна влучати часто й дрібно, а промахуватись рідко й дорого
        let Some(nm) = null_med else { no_null += 1; continue };
        let ex = o.excess_pct.unwrap_or(0.0) - nm;
        let bucket = match s.strength["emis_pct"].as_f64().unwrap_or(0.0) {
            p if p <= 0.0 => "емісія невідома",
            p if p < 0.05 => "<0.05% емісії",
            p if p < 0.20 => "0.05–0.2% емісії",
            _ => ">0.2% емісії",
        };
        by_emis.entry((s.source.clone(), s.kind.clone(), o.horizon_d, bucket)).or_default().push(ex);
    }

    let first_ts = signals.iter().map(|s| s.ts).min().unwrap_or(now);
    let pending: usize = signals.iter().map(|s| horizons.iter()
        .filter(|&&h| s.ts + h as i64 * 86_400 <= now && !j.has_outcome(&s.id, h)).count()).sum();

    let mut t = format!(
        "📒 <b>ЖУРНАЛ СИГНАЛІВ</b> — перевірка на {}\n\
         записів {} від {} · результатів {} · чекають {}\n\
         <i>надлишок = дохідність активу мінус {} за те саме вікно</i>\n",
        ymd(now), signals.len(), ymd(first_ts), outcomes.len(), pending,
        bench.split(':').nth(1).unwrap_or("ринок").to_uppercase());

    let mut keys: Vec<&(String, String, u32)> = cells.keys().collect();
    keys.sort_by(|a, b| (&a.0, &a.1, a.2).cmp(&(&b.0, &b.1, b.2)));
    for k in keys {
        let hits = &cells[k];
        let n = hits.len();
        let rate = 100.0 * hits.iter().sum::<f64>() / n as f64;
        let mut ex: Vec<f64> = cell_ex.get(k).cloned().unwrap_or_default();
        let med = median(&mut ex);
        let word = if k.1 == "sell" { "падіння" } else { "зростання" };
        if n < min_n {
            t.push_str(&format!("\n<b>{} · {} · {}д</b>: n={n} — мало спостережень, присуду немає (треба {min_n})\n",
                k.0, k.1, k.2));
            continue
        }
        // ── з чим порівнюємо ──
        // Не з 50%, а з тим, що дав би випадковий момент входу в ТІ САМІ активи на тому
        // самому горизонті (нульова база з реплею, зважена за кількістю сигналів на актив).
        // Актив, який весь період обганяв ринок, дає «влучання» на будь-якій даті — і саме
        // це порівняння знімає таку ілюзію.
        let syms = cell_syms.get(k).cloned().unwrap_or_default();
        let (mut w_sum, mut null_hit_w, mut null_med_w) = (0.0f64, 0.0f64, 0.0f64);
        // Нуль шукаємо за таймфреймом джерела («retro-1h» → «1h»): база для годинних
        // відер і для п'ятихвилинних — різні величини, плутати їх не можна.
        let tf = k.0.strip_prefix("retro-").unwrap_or("");
        for (sym, cnt) in &syms {
            let Some(nb) = null_base.pointer(&format!("/by_tf/{tf}/{sym}/{}", k.2)) else { continue };
            let (Some(up), Some(m)) = (nb["share_up"].as_f64(), nb["median_excess"].as_f64()) else { continue };
            let w = *cnt as f64;
            w_sum += w;
            null_hit_w += w * if k.1 == "sell" { 100.0 - up } else { up };
            null_med_w += w * if k.1 == "sell" { -m } else { m };
        }
        let covered = if syms.is_empty() { 0.0 } else { 100.0 * w_sum / n as f64 };
        let base_rate = if w_sum > 0.0 { null_hit_w / w_sum } else { 50.0 };
        let base_med = if w_sum > 0.0 { null_med_w / w_sum } else { 0.0 };

        // Похибка вважається на КІЛЬКОСТІ АКТИВІВ, а не сигналів: вікна сусідніх сигналів
        // одного токена перекриваються майже повністю, тож 400 спостережень по двох
        // активах — це не 400 незалежних дослідів, а приблизно два.
        let eff = syms.len().max(1) as f64;
        let se = 50.0 / eff.sqrt();
        let lift = rate - base_rate;
        let verdict = if w_sum <= 0.0 {
            "нульової бази для цих активів немає — порівнювати нема з чим".to_string()
        } else if lift.abs() > 2.0 * se {
            if lift > 0.0 { format!("краще за випадковий вхід на {:+.0} пп", lift) }
            else { format!("ГІРШЕ за випадковий вхід на {lift:+.0} пп (сигнал інвертований?)") }
        } else {
            format!("те саме, що випадковий вхід ({lift:+.0} пп при межі ±{:.0})", 2.0 * se)
        };

        // Найбільший актив у вибірці: якщо він дає більшість сигналів, це висновок про нього,
        // а не про сигнал взагалі.
        let top = syms.iter().max_by_key(|(_, c)| **c).map(|(s, c)| (s.clone(), 100.0 * *c as f64 / n as f64));

        t.push_str(&format!(
            "\n<b>{} · {} · {}д</b>: n={n} · вгадано {word} {:.0}% · медіана надлишку {:+.2}%\n\
             випадковий вхід у ті самі активи: {:.0}% · {:+.2}%\n{}\n",
            k.0, k.1, k.2, rate, med, base_rate, base_med, verdict));
        if let Some((sym, share)) = top {
            t.push_str(&format!("   активів {} · найбільший {sym} дає {share:.0}% вибірки\n", syms.len()));
        }
        if covered < 95.0 && w_sum > 0.0 {
            t.push_str(&format!("   ⚠ нульова база покриває лише {covered:.0}% вибірки\n"));
        }

        // зріз за часткою емісії — там, де спостережень вистачає
        let mut bkeys: Vec<&&'static str> = by_emis.keys()
            .filter(|(s, kd, h, _)| (s, kd, h) == (&k.0, &k.1, &k.2)).map(|(_, _, _, b)| b).collect();
        bkeys.sort();
        bkeys.dedup();
        for b in bkeys {
            let mut v = by_emis[&(k.0.clone(), k.1.clone(), k.2, *b)].clone();
            if v.len() < min_n / 3 { continue }
            t.push_str(&format!("   {b}: n={} · понад випадковий вхід {:+.2}%\n", v.len(), median(&mut v)));
        }
    }

    if unresolved > 0 { t.push_str(&format!("\n⚠ без ціни: {unresolved} результатів\n")) }
    if self_bench > 0 {
        t.push_str(&format!("ℹ виключено як базовий актив ({bench_sym}): {self_bench} результатів\n"));
    }
    if no_null > 0 {
        t.push_str(&format!("⚠ без нульової бази (зрізи за емісією їх не враховують): {no_null}\n"));
    }
    if j.bad_lines() > 0 { t.push_str(&format!("⚠ битих рядків у журналі: {}\n", j.bad_lines())) }
    if cells.is_empty() {
        t.push_str("\nЖодної дозрілої пари ще немає — найближчий присуд буде, коли мине перший горизонт.\n");
    }
    if cells.keys().any(|(s, _, _)| s.starts_with("retro")) {
        t.push_str("\n<i>Ретро-рядки: вибірка — активи, які групи тримають СЬОГОДНІ (виживання), \
                    а частка емісії рахована з сьогоднішнього випуску (зазирання вперед). \
                    Вікна сусідніх сигналів перекриваються, тому похибка рахується на кількості \
                    активів, а не сигналів.</i>\n");
    }
    t.push_str("\n<i>Журнал дописується, а не переписується: запис про сигнал фізично старший за запис про результат.</i>");

    println!("{}", t.replace("<b>", "").replace("</b>", "").replace("<i>", "").replace("</i>", ""));
    if !dry && mode != "mature" {
        let token = envv(&root, "TELEGRAM_BOT_TOKEN");
        let chat = envv(&root, "TELEGRAM_CHAT_ID");
        if tg(&client, &token, &chat, &t).await { eprintln!("[sigcheck] звіт надіслано") }
    }
}
