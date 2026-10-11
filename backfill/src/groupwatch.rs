//! groupwatch — алерт РІВНЯ ГРУПИ: коли група капіталу міняє намір щодо активу.
//!
//! ЧОМУ ЦЕ ОКРЕМА РІЧ. Усі інструменти ринку працюють з адресою або сутністю. Ніхто не
//! робить алерт на рівні ВИЯВЛЕНОЇ ГРУПИ: «десятки гаманців, які ми звели в одну групу,
//! разом почали роздавати цей актив». Саме це тут і відстежується — не окремий переказ,
//! а зміна наміру групи, виміряна часткою емісії.
//!
//! ЩО ВВАЖАЄТЬСЯ ПОДІЄЮ:
//!   * фаза активу в групі перекинулась (НАБИРАЄ/ТРИМАЄ → РОЗДАЄ і навпаки);
//!   * частка емісії під контролем групи змінилась більше ніж на `MIN_DELTA_PP`.
//! Решта коливань — шум, і мовчати про них правильно.
//!
//! КОЖЕН АЛЕРТ ОДРАЗУ ЙДЕ В ЖУРНАЛ (`wr_store::journal`). Це головне, чим цей монітор
//! відрізняється від дев'яти попередніх: він народжується вже перевірюваним. Через
//! 1/7/30/90 днів `sigcheck` порахує, що сталося з ціною, і в послужному списку
//! з'явиться рядок «groupwatch» — без жодної додаткової роботи. Монітор, який не можна
//! перевірити, у цьому проєкті більше не заводиться.
//!
//! ПЕРШИЙ ЗАПУСК МОВЧИТЬ. Він лише знімає базовий стан: інакше під час першого прогону
//! прилетіло б кілька сотень «змін», яких насправді не було.
//!
//! Args: ROOT. Env: MIN_DELTA_PP(0.25) MIN_USD(50000) TOP(12) DRY(0)

use serde_json::{json, Value};
use std::fs;

fn env_f(k: &str, d: f64) -> f64 { std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d) }
fn env_u(k: &str, d: u64) -> u64 { std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d) }

fn now_s() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Креденшли Telegram — у `<ROOT>/.env`, як у решти бінів репозиторію.
fn envv(root: &str, k: &str) -> String {
    fs::read_to_string(format!("{root}/.env")).unwrap_or_default().lines()
        .find(|l| l.starts_with(&format!("{k}=")))
        .map(|l| l.splitn(2, '=').nth(1).unwrap_or("").trim().to_string())
        .unwrap_or_default()
}

async fn tg(client: &reqwest::Client, token: &str, chat: &str, text: &str) -> bool {
    if token.is_empty() || chat.is_empty() { eprintln!("[groupwatch] нема TELEGRAM креденшлів"); return false }
    matches!(client.post(format!("https://api.telegram.org/bot{token}/sendMessage"))
        .form(&[("chat_id", chat), ("text", text), ("parse_mode", "HTML"), ("disable_web_page_preview", "true")])
        .send().await, Ok(r) if r.status().is_success())
}

fn usd_fmt(v: f64) -> String {
    let a = v.abs();
    if a >= 1e9 { format!("${:.2} млрд", a / 1e9) }
    else if a >= 1e6 { format!("${:.1} млн", a / 1e6) }
    else { format!("${:.0} тис.", a / 1e3) }
}

/// Чи фаза означає роздачу. Рядки приходять із `groupsig` українською.
fn distributing(phase: &str) -> bool { phase.contains("РОЗДА") }

/// Що саме сталося з позицією.
#[derive(PartialEq, Clone, Copy)]
enum Change { StartedSelling, StoppedSelling, ShareUp, ShareDown }

impl Change {
    fn icon(self) -> &'static str {
        match self {
            Change::StartedSelling => "🔴",
            Change::StoppedSelling => "🟢",
            Change::ShareUp => "📈",
            Change::ShareDown => "📉",
        }
    }
    fn text(self) -> &'static str {
        match self {
            Change::StartedSelling => "ПОЧАЛА РОЗДАВАТИ",
            Change::StoppedSelling => "припинила роздавати",
            Change::ShareUp => "наростила частку емісії",
            Change::ShareDown => "зменшила частку емісії",
        }
    }
    /// Напрям твердження для журналу: роздача групи — це ставка на слабкість.
    fn kind(self) -> &'static str {
        match self {
            Change::StartedSelling | Change::ShareDown => "sell",
            Change::StoppedSelling | Change::ShareUp => "buy",
        }
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let min_delta = env_f("MIN_DELTA_PP", 0.25);
    let min_usd = env_f("MIN_USD", 50_000.0);
    let top = env_u("TOP", 12) as usize;
    let dry = env_u("DRY", 0) == 1;

    let ds = std::env::var("DATASET_DIR").unwrap_or_else(|_| format!("{root}/dataset"));
    let cg_path = format!("{ds}/capital-groups.json");
    let state_path = format!("{ds}/backfill/groupwatch-state.json");

    let cg: Value = match fs::read_to_string(&cg_path).ok().and_then(|s| serde_json::from_str(&s).ok()) {
        Some(v) => v,
        None => { eprintln!("[groupwatch] {cg_path} не прочитано — без нього моніторити нічого"); std::process::exit(2) }
    };
    let prev: Value = fs::read_to_string(&state_path).ok()
        .and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null);
    let first_run = prev.is_null();

    let empty = vec![];
    let now = now_s();
    let mut state = serde_json::Map::new();
    let mut changes: Vec<(Change, String, Value)> = Vec::new(); // (що, група, позиція)
    let mut scanned = 0usize;
    let mut skipped = 0usize;

    for g in cg["groups"].as_array().unwrap_or(&empty) {
        let gid = g["id"].as_str().unwrap_or("").to_string();
        let sigs = match g["tokenSignals"].as_array() { Some(s) if !s.is_empty() => s, _ => continue };

        for t in sigs {
            // Ті самі запобіжники, що в /api/exit-pressure: позиції з поламаною
            // десятковою точністю або без ціни не породжують алертів.
            if t["unpriceable"].as_bool() == Some(true) { skipped += 1; continue }
            let usd = t["usd"].as_f64().unwrap_or(0.0);
            if !usd.is_finite() || usd < min_usd || usd > 5e10 { skipped += 1; continue }
            let Some(emis) = t["emissionPct"].as_f64().filter(|v| v.is_finite() && *v > 0.0) else { skipped += 1; continue };
            let contract = t["token"].as_str().unwrap_or("").to_string();
            if contract.is_empty() { skipped += 1; continue }
            scanned += 1;

            let phase = t["phase"].as_str().unwrap_or("").to_string();
            let key = format!("{gid}|{}", contract.to_lowercase());
            let p_phase = prev.pointer(&format!("/{}/phase", key.replace('/', "~1")))
                .and_then(|v| v.as_str()).map(|s| s.to_string());
            let p_emis = prev.pointer(&format!("/{}/emissionPct", key.replace('/', "~1")))
                .and_then(|v| v.as_f64());

            state.insert(key.clone(), json!({
                "phase": phase, "emissionPct": emis, "usd": usd, "ts": now,
                "symbol": t["symbol"].as_str().unwrap_or(""),
            }));

            if first_run { continue }

            // 1) перекид фази — найважливіша подія
            if let Some(pp) = p_phase.as_deref() {
                let was = distributing(pp);
                let is = distributing(&phase);
                if is && !was {
                    changes.push((Change::StartedSelling, gid.clone(), t.clone()));
                    continue;
                }
                if was && !is {
                    changes.push((Change::StoppedSelling, gid.clone(), t.clone()));
                    continue;
                }
            }
            // 2) істотна зміна частки емісії
            if let Some(pe) = p_emis {
                let d = emis - pe;
                if d.abs() >= min_delta {
                    changes.push((if d > 0.0 { Change::ShareUp } else { Change::ShareDown }, gid.clone(), t.clone()));
                }
            }
        }
    }

    // ── журнал: кожен алерт стає перевірюваним твердженням ──
    let mut logged = 0usize;
    if !dry && !changes.is_empty() {
        match wr_store::journal::Journal::open_at(&ds) {
            Ok(mut j) => {
                let mut batch = Vec::new();
                for (ch, gid, t) in &changes {
                    let usd = t["usd"].as_f64().unwrap_or(0.0);
                    let holdings = t["holdings"].as_f64().unwrap_or(0.0);
                    // Ціна виводиться з позиції: журнал не приймає запис без ціни,
                    // бо такий запис неперевірний.
                    let price = if holdings > 0.0 { usd / holdings } else { 0.0 };
                    if !(price > 0.0) || !price.is_finite() { continue }
                    let sym = t["symbol"].as_str().unwrap_or("?");
                    let contract = t["token"].as_str().unwrap_or("").to_lowercase();
                    let mut rec = wr_store::journal::signal(
                        "groupwatch", ch.kind(), sym, now, now, "onchain", &contract, price,
                        json!({
                            "group": gid,
                            "emis_pct": t["emissionPct"].as_f64(),
                            "usd": usd,
                            "daysOfVolume": t["daysOfVolume"].as_f64(),
                            "phase": t["phase"].as_str().unwrap_or(""),
                            "change": ch.text(),
                            // Денний крок: саме за цим ключем послужний список знайде
                            // нульову базу (`by_tf/1d`, яку рахує бін `nullbase` з
                            // локальних цінових рядів). Без неї присуд неможливий.
                            "tf": "1d",
                        }),
                    );
                    rec.contract = contract;
                    rec.chain = "ethereum".into();
                    batch.push(rec);
                }
                match j.append_many(&batch) {
                    Ok(n) => logged = n,
                    Err(e) => eprintln!("[groupwatch] журнал не прийняв пачку: {e}"),
                }
            }
            Err(e) => eprintln!("[groupwatch] журнал недоступний: {e} — алерти підуть, але перевірити їх буде нічим"),
        }
    }

    // ── стан ──
    // Пишемо ПІСЛЯ журналу: якщо процес загине між ними, наступний прогін повторить
    // алерт (це видно), а не втратить його назавжди (це не видно).
    let _ = fs::create_dir_all(format!("{ds}/backfill"));
    if let Err(e) = fs::write(&state_path, serde_json::to_string_pretty(&Value::Object(state)).unwrap_or_default()) {
        eprintln!("[groupwatch] стан не записано: {e} — наступний прогін повторить ці ж алерти");
    }

    // ── повідомлення ──
    if first_run {
        eprintln!("[groupwatch] перший запуск: знято базовий стан по {scanned} позиціях (пропущено {skipped}), алертів немає");
        return;
    }
    eprintln!("[groupwatch] позицій {scanned} (пропущено {skipped}) · змін {} · у журнал {logged}", changes.len());
    if changes.is_empty() { return }

    // Найважливіше вгорі: спершу початок роздачі, далі за часткою емісії.
    changes.sort_by(|a, b| {
        let rank = |c: Change| match c { Change::StartedSelling => 0, Change::ShareDown => 1, Change::ShareUp => 2, Change::StoppedSelling => 3 };
        rank(a.0).cmp(&rank(b.0)).then(
            b.2["emissionPct"].as_f64().unwrap_or(0.0)
                .partial_cmp(&a.2["emissionPct"].as_f64().unwrap_or(0.0)).unwrap_or(std::cmp::Ordering::Equal))
    });

    let mut msg = format!("👥 <b>ГРУПИ КАПІТАЛУ — зміна наміру</b>\n<i>подій: {}</i>\n", changes.len());
    for (ch, gid, t) in changes.iter().take(top) {
        let sym = t["symbol"].as_str().unwrap_or("?");
        let emis = t["emissionPct"].as_f64().unwrap_or(0.0);
        let usd = t["usd"].as_f64().unwrap_or(0.0);
        let dov = t["daysOfVolume"].as_f64().unwrap_or(-1.0);
        let vol24 = t["vol24"].as_f64().unwrap_or(0.0);
        // «120 796 днів обороту» — не вимірювання, а ділення на майже-нуль. Коли
        // добовий оборот нижчий за $1 тис., ринку фактично немає, і саме це треба
        // написати, а не показувати астрономічне число як факт.
        let exit = if vol24 < 1_000.0 {
            " · <b>ринку немає</b> — вийти нікуди".to_string()
        } else if dov >= 0.0 {
            format!(" · на вихід {:.0} дн обороту", dov)
        } else { String::new() };
        msg.push_str(&format!("\n{} <b>{sym}</b> · {gid}\n{} · {:.2}% емісії · {}{}\n",
            ch.icon(), ch.text(), emis, usd_fmt(usd), exit));
    }
    msg.push_str("\n<i>Кожна з цих подій записана в журнал і буде перевірена через 1/7/30/90 днів.</i>");

    println!("{}", msg.replace("<b>", "").replace("</b>", "").replace("<i>", "").replace("</i>", ""));
    if dry { return }
    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(25)).build().unwrap();
    let (token, chat) = (envv(&root, "TELEGRAM_BOT_TOKEN"), envv(&root, "TELEGRAM_CHAT_ID"));
    if tg(&client, &token, &chat, &msg).await { eprintln!("[groupwatch] надіслано") }
}
