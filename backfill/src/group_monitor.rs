//! group_monitor (Rust) — порт deploy/group-monitor.py 1:1.
//! Моніторинг 2 трекінг-груп: щодня докраулює свіжі eth ERC-20 трансфери ядер груп,
//! рахує зміну net-холдингу/емісії по priced-активах, шле Telegram-дайджест по кожній групі.
//! Перший запуск = baseline із transfers.csv (без алерту). Далі = дельта since last_block.
//! Стан: dataset/backfill/group-monitor-state.json. Cron щодня 08:15.
//! Env (дефолти = поведінка Python): ENDBLOCK(99999999), DRY(непорожнє — не слати TG,
//! лише друкувати), STATE/GROUPS (перевизначення шляхів; для parity-прогонів),
//! TRIES(6), SLEEP_MS(120) — див. нижче.
//!
//! СВІДОМЕ ВІДХИЛЕННЯ ВІД PYTHON-ОРИГІНАЛУ (2026-07-29, за рішенням користувача).
//! Оригінал ретраїв лише на HTTP-виняток, а відповідь `{"result":"Max rate limit reached"}`
//! мовчки трактував як «нуль трансферів». Оскільки `last_block` просувався попри це,
//! пропущені гаманці губили свої дельти НАЗАВЖДИ. Тут:
//!   1. `es_list` ретраїть, доки `result` не стане масивом (TRIES спроб, ротація ключів);
//!   2. пауза SLEEP_MS між гаманцями — ключі спільні з backfill2/cexsignal, це знімає
//!      більшість 429-х ще до ретраїв;
//!   3. курсор тепер ОКРЕМИЙ НА КОЖЕН ГАМАНЕЦЬ (`cursors` у стані, як у relay-monitor).
//!      Успішно прокраулений гаманець просуває свій курсор; той, що не піддався, лишає
//!      старий і буде дочитаний наступного запуску. Так один відсталий не блокує решту,
//!      і жодна дельта не губиться та не подвоюється.
//! Стан зворотно сумісний: якщо `cursors` немає, усі гаманці сідають на `last_block`.
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Порядок груп фіксований — дайджест друкується A, потім B (як у Python-дикті).
const GROUP_IDS: [&str; 2] = ["A_metanet", "B_stocks"];

fn group_name(id: &str) -> &'static str {
    match id {
        "A_metanet" => "A · мета-мережа (#0+#2+#3)",
        _ => "B · ток.акції (#1)",
    }
}

fn envv(root: &str, k: &str) -> String {
    fs::read_to_string(format!("{root}/.env"))
        .unwrap_or_default()
        .lines()
        .find(|l| l.starts_with(&format!("{k}=")))
        .map(|l| l.splitn(2, '=').nth(1).unwrap_or("").trim().to_string())
        .unwrap_or_default()
}

/// `es(params)` — Etherscan V2 з ротацією ключів, 3 спроби, пауза 0.4с, таймаут 25с.
async fn es(client: &reqwest::Client, keys: &[String], kc: &AtomicUsize, params: &str) -> Value {
    for _ in 0..3 {
        let key = &keys[kc.fetch_add(1, Ordering::Relaxed) % keys.len()];
        let url = format!("https://api.etherscan.io/v2/api?chainid=1&{params}&apikey={key}");
        if let Ok(r) = client.get(&url).send().await {
            if let Ok(v) = r.json::<Value>().await {
                return v;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    }
    json!({})
}

/// Як `es`, але вимагає, щоб `result` був МАСИВОМ. Рядок замість масиву — це
/// `"Max rate limit reached"` / `"NOTOK"`, тобто збій, а не «нуль трансферів»:
/// ретраїмо з наступним ключем і зростаючою паузою. `None` = дані здобути не вдалося.
async fn es_list(
    client: &reqwest::Client,
    keys: &[String],
    kc: &AtomicUsize,
    params: &str,
    tries: usize,
) -> Option<Vec<Value>> {
    for attempt in 0..tries {
        let v = es(client, keys, kc, params).await;
        if let Some(a) = v["result"].as_array() {
            return Some(a.clone());
        }
        if attempt + 1 < tries {
            // лінійний backoff 0.5с, 1.0с, 1.5с… — ключі спільні, під навантаженням
            // backfill2/cexsignal коротких пауз не вистачає
            let back = 500 * (attempt as u64 + 1);
            tokio::time::sleep(std::time::Duration::from_millis(back)).await;
        }
    }
    None
}

async fn tg(client: &reqwest::Client, token: &str, chat: &str, text: &str, dry: bool) {
    if token.is_empty() || chat.is_empty() || dry {
        let head: String = text.chars().take(200).collect();
        println!("[tg skip] {head}");
        return;
    }
    let url = format!("https://api.telegram.org/bot{token}/sendMessage");
    if let Err(e) = client
        .post(&url)
        .form(&[("chat_id", chat), ("text", text), ("parse_mode", "HTML")])
        .send()
        .await
    {
        println!("[tg err] {e}");
    }
}

async fn cur_block(client: &reqwest::Client, keys: &[String], kc: &AtomicUsize) -> u64 {
    let r = es(client, keys, kc, "module=proxy&action=eth_blockNumber").await;
    let hex = r["result"].as_str().unwrap_or("0x0");
    u64::from_str_radix(hex.trim_start_matches("0x"), 16).unwrap_or(0)
}

/// Число з JSON незалежно від того, рядок це чи число (Python `float(x)`/`int(x)`).
/// Порожній рядок → None, щоб виклик міг узяти дефолт (як `x or 18` у Python).
fn num(v: &Value) -> Option<f64> {
    match v {
        Value::String(s) if !s.trim().is_empty() => s.trim().parse().ok(),
        Value::Number(n) => n.as_f64(),
        _ => None,
    }
}

fn read_json(path: &str) -> Value {
    let mut s = String::new();
    match fs::File::open(path) {
        Ok(f) => {
            let _ = BufReader::new(f).read_to_string(&mut s);
            serde_json::from_str(&s).unwrap_or(Value::Null)
        }
        Err(_) => Value::Null,
    }
}

fn holdings_to_json(hold: &HashMap<&str, BTreeMap<String, f64>>) -> Value {
    let mut out = Map::new();
    for g in GROUP_IDS {
        let mut m = Map::new();
        for (tok, v) in &hold[g] {
            m.insert(tok.clone(), json!(v));
        }
        out.insert(g.to_string(), Value::Object(m));
    }
    Value::Object(out)
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let ds = format!("{root}/dataset/backfill");
    let statef = std::env::var("STATE").unwrap_or_else(|_| format!("{ds}/group-monitor-state.json"));
    let groupsf = std::env::var("GROUPS").unwrap_or_else(|_| format!("{ds}/groups-tracked.json"));
    let endblock = std::env::var("ENDBLOCK").unwrap_or_else(|_| "99999999".into());
    let dry = std::env::var("DRY").map(|v| !v.is_empty()).unwrap_or(false);
    let tries: usize = std::env::var("TRIES").ok().and_then(|s| s.parse().ok()).unwrap_or(10);
    let sleep_ms: u64 = std::env::var("SLEEP_MS").ok().and_then(|s| s.parse().ok()).unwrap_or(120);

    let keys: Vec<String> = envv(&root, "BACKFILL_ETHERSCAN_KEYS")
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let (token, chat) = (envv(&root, "TELEGRAM_BOT_TOKEN"), envv(&root, "TELEGRAM_CHAT_ID"));
    let kc = AtomicUsize::new(0);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(25))
        .build()
        .unwrap();

    // priced eth + остання ціна
    let mut priced: HashMap<String, f64> = HashMap::new();
    if let Ok(rd) = fs::read_dir(format!("{root}/dataset/nextaction/prices")) {
        for e in rd.flatten() {
            let fname = e.file_name().to_string_lossy().to_string();
            if !fname.starts_with("eth_") || !fname.ends_with(".json") {
                continue;
            }
            let c = fname[4..fname.len() - 5].to_lowercase();
            let d = read_json(&e.path().to_string_lossy());
            if let Some(ps) = d["prices"].as_array() {
                if let Some(last) = ps.last() {
                    priced.insert(c, last["price"].as_f64().unwrap_or(0.0));
                }
            }
        }
    }

    // supply → (кількість у токенах, символ)
    let mut sup: HashMap<String, (f64, String)> = HashMap::new();
    if let Some(obj) = read_json(&format!("{root}/dataset/token-supply.json")).as_object() {
        for (c, v) in obj {
            let dec = v["decimals"].as_i64().unwrap_or(18) as i32;
            let raw = match &v["supply"] {
                Value::String(s) => s.trim().parse::<f64>().unwrap_or(0.0),
                Value::Number(n) => n.as_f64().unwrap_or(0.0),
                _ => 0.0,
            };
            if raw > 0.0 {
                let symbol = v["symbol"]
                    .as_str()
                    .map(String::from)
                    .unwrap_or_else(|| c.chars().take(8).collect());
                sup.insert(c.to_lowercase(), (raw / 10f64.powi(dec), symbol));
            }
        }
    }

    // групи (будуємо з validated-desks якщо нема)
    if fs::metadata(&groupsf).is_err() {
        let vd = read_json(&format!("{ds}/validated-desks.json"));
        let mut a: Vec<String> = Vec::new();
        let mut b: Vec<String> = Vec::new();
        if let Some(desks) = vd["validated_desks"].as_array() {
            for d in desks {
                let dst = if d["size"].as_i64() == Some(8) { &mut b } else { &mut a };
                if let Some(ms) = d["members"].as_array() {
                    for m in ms {
                        if let Some(s) = m.as_str() {
                            dst.push(s.to_lowercase());
                        }
                    }
                }
            }
        }
        for v in [&mut a, &mut b] {
            v.sort();
            v.dedup();
        }
        let _ = fs::write(
            &groupsf,
            serde_json::to_string(&json!({"A_metanet": a, "B_stocks": b})).unwrap(),
        );
    }
    let g = read_json(&groupsf);
    let groups: HashMap<&str, Vec<String>> = GROUP_IDS
        .iter()
        .map(|&id| {
            let ms = g[id]
                .as_array()
                .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default();
            (id, ms)
        })
        .collect();
    let mut w2g: HashMap<String, &str> = HashMap::new();
    for &id in GROUP_IDS.iter() {
        for w in &groups[id] {
            w2g.insert(w.clone(), id);
        }
    }
    // детермінований порядок обходу (Python мав set — порядок довільний, сума та сама)
    let mut allw: Vec<String> = w2g.keys().cloned().collect();
    allw.sort();
    println!(
        "групи: A={}, B={} ядер ({} унік.)",
        groups["A_metanet"].len(),
        groups["B_stocks"].len(),
        allw.len()
    );

    let mut hold: HashMap<&str, BTreeMap<String, f64>> =
        GROUP_IDS.iter().map(|&id| (id, BTreeMap::new())).collect();

    // ── BASELINE mode ────────────────────────────────────────────────────────
    if fs::metadata(&statef).is_err() {
        if keys.is_empty() {
            eprintln!("[group-monitor] нема BACKFILL_ETHERSCAN_KEYS");
            return;
        }
        let f = fs::File::open(format!("{ds}/transfers.csv")).expect("transfers.csv");
        let mut rdr = BufReader::with_capacity(1 << 20, f);
        let mut line = String::new();
        let _ = rdr.read_line(&mut line); // заголовок
        loop {
            line.clear();
            // read_line вимагає коректний UTF-8; на першому ж непридатному байті
            // повертає помилку, і обрив тут означав мовчазну втрату решти файлу.
            match rdr.read_line(&mut line) {
                Ok(0) => break,
                Err(e) if e.kind() == std::io::ErrorKind::InvalidData => continue,
                Err(_) => break,
                Ok(_) => {}
            }
            let p: Vec<&str> = line.splitn(6, ',').collect();
            if p.len() < 5 || p[1] != "eth" {
                continue;
            }
            let w = p[0].to_lowercase();
            let Some(&gid) = w2g.get(&w) else { continue };
            let tok = p[4].to_lowercase();
            if !priced.contains_key(&tok) {
                continue;
            }
            // amount = передостаннє поле (rsplit(",", 2)[1])
            let mut tail = line.rsplitn(3, ',');
            let _tx = tail.next();
            let amt: f64 = tail.next().unwrap_or("").trim().parse().unwrap_or(0.0);
            let signed = if p[2] == "i" { amt } else { -amt };
            *hold.get_mut(gid).unwrap().entry(tok).or_insert(0.0) += signed;
        }
        let blk = cur_block(&client, &keys, &kc).await;
        let cur_json: Map<String, Value> = allw.iter().map(|w| (w.clone(), json!(blk))).collect();
        let _ = fs::write(
            &statef,
            serde_json::to_string(
                &json!({"last_block": blk, "cursors": cur_json, "holdings": holdings_to_json(&hold)}),
            )
            .unwrap(),
        );
        tg(
            &client,
            &token,
            &chat,
            &format!("📊 Group-monitor BASELINE встановлено. Група A (мета-мережа) та B (ток.акції) відстежуються з блоку {blk}."),
            dry,
        )
        .await;
        println!("baseline set, last_block {blk}");
        return;
    }

    // ── MONITOR mode ─────────────────────────────────────────────────────────
    if keys.is_empty() {
        eprintln!("[group-monitor] нема BACKFILL_ETHERSCAN_KEYS");
        return;
    }
    let st = read_json(&statef);
    let lastblk = st["last_block"].as_u64().unwrap_or(0);
    for &id in GROUP_IDS.iter() {
        if let Some(m) = st["holdings"][id].as_object() {
            for (k, v) in m {
                hold.get_mut(id).unwrap().insert(k.clone(), v.as_f64().unwrap_or(0.0));
            }
        }
    }
    let mut delta: HashMap<&str, BTreeMap<String, f64>> =
        GROUP_IDS.iter().map(|&id| (id, BTreeMap::new())).collect();
    let mut maxblk = lastblk;

    // Курсор кожного гаманця; за відсутності `cursors` (старий формат) — глобальний last_block.
    let mut cursors: HashMap<String, u64> = HashMap::new();
    for w in &allw {
        let c = st["cursors"][w].as_u64().unwrap_or(lastblk);
        cursors.insert(w.clone(), c);
    }

    let mut failed: Vec<&String> = Vec::new();
    let mut ok: Vec<&String> = Vec::new();
    for (idx, w) in allw.iter().enumerate() {
        if idx > 0 && sleep_ms > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(sleep_ms)).await;
        }
        let from = cursors[w] + 1;
        let txs = es_list(
            &client,
            &keys,
            &kc,
            &format!(
                "module=account&action=tokentx&address={w}&startblock={from}&endblock={endblock}&page=1&offset=1000&sort=asc"
            ),
            tries,
        )
        .await;
        let Some(txs) = txs else {
            eprintln!("[group-monitor] не прокраулено після {tries} спроб: {w}");
            failed.push(w);
            continue;
        };
        ok.push(w);
        for t in &txs {
            // Etherscan віддає числа рядками, але Python-оригінал прийняв би й JSON-число.
            let bn = num(&t["blockNumber"]).unwrap_or(0.0) as u64;
            if bn > maxblk {
                maxblk = bn;
            }
            let tok = t["contractAddress"].as_str().unwrap_or("").to_lowercase();
            if !priced.contains_key(&tok) {
                continue;
            }
            let dec = num(&t["tokenDecimal"]).map(|d| d as i32).unwrap_or(18);
            let Some(amt) = num(&t["value"]) else { continue };
            let amt = amt / 10f64.powi(dec);
            let gid = w2g[w];
            let to = t["to"].as_str().unwrap_or("").to_lowercase();
            let frm = t["from"].as_str().unwrap_or("").to_lowercase();
            let signed = if to == *w {
                amt
            } else if frm == *w {
                -amt
            } else {
                continue;
            };
            *hold.get_mut(gid).unwrap().entry(tok.clone()).or_insert(0.0) += signed;
            *delta.get_mut(gid).unwrap().entry(tok).or_insert(0.0) += signed;
        }
    }

    // Успішний обхід означає, що для цього гаманця прочитано ВСЕ від його курсора і далі,
    // тож його курсор можна підняти до найвищого баченого блоку. Невдалі лишаються на місці.
    for w in &ok {
        cursors.insert((*w).clone(), maxblk);
    }
    if !failed.is_empty() {
        eprintln!(
            "[group-monitor] {}/{} гаманців не прокраулено — їхні курсори не рухаю, дочитаю наступного разу",
            failed.len(),
            allw.len()
        );
    }

    // дайджест
    let mut lines: Vec<String> = Vec::new();
    for &gid in GROUP_IDS.iter() {
        let mut movers: Vec<(&String, f64, f64)> = delta[gid]
            .iter()
            .map(|(tok, &d)| (tok, d, d * priced.get(tok).copied().unwrap_or(0.0)))
            .filter(|(_, _, usd)| usd.abs() >= 1000.0)
            .collect();
        movers.sort_by(|a, b| b.2.abs().partial_cmp(&a.2.abs()).unwrap_or(std::cmp::Ordering::Equal));
        if movers.is_empty() {
            continue;
        }
        lines.push(format!("<b>{}</b>", group_name(gid)));
        for (tok, d, usd) in movers.into_iter().take(8) {
            let fallback: String = tok.chars().take(8).collect();
            let (hs, symb) = match sup.get(tok) {
                Some((h, s)) => (*h, s.clone()),
                None => (0.0, fallback),
            };
            let arrow = if d > 0.0 { "🟢 набирає" } else { "🔴 роздає" };
            let em = if hs > 0.0 {
                format!(" | емісія {:.1}%", hold[gid][tok] / hs * 100.0)
            } else {
                String::new()
            };
            lines.push(format!("  {arrow} {symb}: {d:+.0} ({usd:+.0}$){em}"));
        }
    }

    let cur_json: Map<String, Value> = cursors.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
    let _ = fs::write(
        &statef,
        serde_json::to_string(
            &json!({"last_block": maxblk, "cursors": cur_json, "holdings": holdings_to_json(&hold)}),
        )
        .unwrap(),
    );
    let warn = if failed.is_empty() {
        String::new()
    } else {
        format!(
            "\n⚠️ {} із {} гаманців не відповіли (rate-limit) — їх дочитаю наступного запуску",
            failed.len(),
            allw.len()
        )
    };
    if !lines.is_empty() {
        tg(
            &client,
            &token,
            &chat,
            &format!(
                "📡 Трекінг-групи, зміни за добу (блоки {lastblk}→{maxblk}):\n{}{warn}",
                lines.join("\n")
            ),
            dry,
        )
        .await;
        println!("digest sent");
    } else {
        println!("no significant moves (blocks {lastblk}->{maxblk})");
    }
}
