//! Q&A assistant — grounds a FREE, OpenAI-compatible LLM (local Ollama qwen2.5:3b
//! by default) in the app's group-level data. To keep a small CPU model accurate,
//! the context ships PRE-COMPUTED aggregates + Ukrainian-labelled phases so the
//! model reads facts instead of counting/translating (which it does poorly).
//!
//! Env: LLM_API_KEY (required to enable), LLM_BASE_URL (default Ollama localhost),
//! LLM_MODEL (default qwen2.5:3b). No key → friendly "not configured".

use axum::{extract::{Query, State}, response::IntoResponse, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

use crate::state::Ctx;

#[derive(Deserialize)]
pub struct AskQuery {
    q: Option<String>,
}

fn uk_phase(f: &Value) -> &'static str {
    match f["phase"].as_str() {
        Some("accumulating") => "набирає",
        Some("distributing") => "роздає",
        Some("neutral") => "нейтрально",
        _ => "невідомо",
    }
}

/// Stablecoins carry no directional signal ("whales bought USDT" is noise), so we
/// strip them from the per-phase coin lists to surface the real bets.
fn is_stable(s: &str) -> bool {
    matches!(s,
        "USDT" | "USDC" | "DAI" | "RLUSD" | "MUSD" | "EURQ" | "USDQ" | "IDRT" | "BRZ"
        | "TUSD" | "FDUSD" | "PYUSD" | "USDE" | "USDD" | "GUSD" | "LUSD" | "FRAX"
        | "EURT" | "EURS" | "USDP" | "SUSD" | "CRVUSD" | "USDB")
}


/// Спам-роздача, замаскована під токен. Такі символи масово розсилаються на тисячі
/// адрес заради фішингу й не несуть жодного сигналу — але потрапляють у топи за
/// частотою саме через масовість.
///
/// Ловимо три ознаки: доменне закінчення (ha138com = ha138.com), рекламні слова
/// та надмірну довжину. Справжні тікери коротші й без цих ознак.
fn is_spam_token(sym: &str) -> bool {
    let s = sym.trim().to_lowercase();
    if s.is_empty() || s.len() > 12 {
        return true;
    }
    const DOMAIN_TAILS: &[&str] = &[
        "com", "net", "org", "xyz", "info", "site", "top", "vip", "club", "app", "io", "cc",
    ];
    // «ha138com» — домен без крапки; але «PROXIMA» не має цифр і не є доменом
    if s.len() >= 5 {
        if let Some(tail) = DOMAIN_TAILS.iter().find(|t| s.ends_with(**t)) {
            let head = &s[..s.len() - tail.len()];
            // хвіст-домен рахуємо спамом лише коли перед ним є цифра або крапка —
            // інакше постраждали б нормальні тікери на кшталт «FILECOIN»
            if head.chars().any(|c| c.is_ascii_digit()) || s.contains('.') {
                return true;
            }
        }
    }
    const AD_WORDS: &[&str] = &[
        "claim", "airdrop", "reward", "visit", "gift", "bonus", "free", "www", "http",
        "promo", "winner", "voucher",
    ];
    if AD_WORDS.iter().any(|w| s.contains(w)) {
        return true;
    }
    // символ із пробілами чи розділовими знаками — теж спам
    !s.chars().all(|c| c.is_ascii_alphanumeric())
}

/// Compact, PRE-AGGREGATED context — the small model reads facts, not computes.
fn build_context(s: &Ctx) -> Value {
    let fams = s.dataset.read_or("families.json", json!({"families": []}));
    let empty = vec![];
    let all = fams["families"].as_array().unwrap_or(&empty);
    let enriched: Vec<&Value> = all.iter().filter(|f| f["transfers"].as_u64().unwrap_or(0) > 0).collect();

    // pre-computed phase aggregates (so the model never has to count)
    let count_phase = |p: &str| enriched.iter().filter(|f| f["phase"].as_str() == Some(p)).count();
    let acc = count_phase("accumulating");
    let dist = count_phase("distributing");
    let neu = count_phase("neutral");
    // full echo-able sentence — the model just repeats this verbatim
    let bigger = if acc > dist {
        "Кити зараз більше НАБИРАЮТЬ (накопичують)."
    } else if dist > acc {
        "Кити зараз більше РОЗДАЮТЬ (продають)."
    } else {
        "Кити набирають і роздають порівну."
    };

    // top non-stable emission tokens per phase — answers the #1 real-world question
    // ("which coins are whales buying / selling") with SYMBOLS, not archetype labels.
    let coins_for = |phase: &str| -> Vec<Value> {
        let mut c: HashMap<String, u64> = HashMap::new();
        for f in enriched.iter().filter(|f| f["phase"].as_str() == Some(phase)) {
            if let Some(arr) = f["topEmission"].as_array() {
                for e in arr.iter().take(3) {
                    if let Some(sym) = e["symbol"].as_str() {
                        if !is_stable(sym) && !is_spam_token(sym) {
                            *c.entry(sym.to_string()).or_default() += 1;
                        }
                    }
                }
            }
        }
        let mut v: Vec<(String, u64)> = c.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0))); // deterministic
        v.into_iter().take(8).map(|(s, n)| json!({"токен": s, "кількістьРодин": n})).collect()
    };
    let coins_acc = coins_for("accumulating");
    let coins_dist = coins_for("distributing");

    // largest enriched family (families.json is sorted by size desc → first)
    let largest = enriched.first().map(|f| {
        json!({
            "розмірГаманців": f["size"],
            "обсягUSD": f["volumeUsd"],
            "фаза": uk_phase(f),
            "домінантнийСектор": f["dominantSector"],
            "архетип": f["archetypeLabel"],
            "топТокенЕмісії": f["topEmission"].get(0).and_then(|e| e["symbol"].as_str()),
        })
    }).unwrap_or(Value::Null);

    // top 5 enriched families, Ukrainian phase (no per-family emission — the
    // монетиЩоНабирають/Роздають lists cover tokens; keeps context lean for the CPU model)
    let top: Vec<Value> = enriched.iter().take(5).map(|f| {
        json!({
            "розмір": f["size"],
            "обсягUSD": f["volumeUsd"],
            "фаза": uk_phase(f),
            "сектор": f["dominantSector"],
            "архетип": f["archetypeLabel"],
        })
    }).collect();

    // net whale flow per narrative — last month + last-3-month trend + direction.
    // "other" is unclassified/stables, not a real narrative → excluded.
    let rot = s.dataset.read_or("sector-rotation.json", json!({}));
    let real_narr = |x: &str| matches!(x, "depin" | "memecoin" | "defi" | "rwa");
    let mut rotation = serde_json::Map::new();
    let mut hottest: (String, f64) = (String::new(), f64::MIN);
    let mut coldest: (String, f64) = (String::new(), f64::MAX);
    if let Some(secs) = rot["sectors"].as_object() {
        for (sec, v) in secs {
            if !real_narr(sec) {
                continue;
            }
            if let Some(arr) = v["net"].as_array() {
                let n = arr.len();
                let last1 = arr.get(n.saturating_sub(1)).and_then(|x| x.as_f64()).unwrap_or(0.0);
                let last3: f64 = arr.iter().rev().take(3).filter_map(|x| x.as_f64()).sum();
                let dir_of = |x: f64| if x > 0.0 {
                    "притік (кити заходять)"
                } else if x < 0.0 {
                    "відтік (кити виходять)"
                } else {
                    "нейтрально"
                };
                rotation.insert(sec.clone(), json!({
                    "останнійМісяцьНетто": last1,
                    "напрямМісяць": dir_of(last1),
                    "останні3місяціНетто": last3,
                    "напрям3місяці": dir_of(last3),
                    "напрям": dir_of(last3),
                }));
                if last3 > hottest.1 {
                    hottest = (sec.clone(), last3);
                }
                if last3 < coldest.1 {
                    coldest = (sec.clone(), last3);
                }
            }
        }
    }
    // Direct, sign-aware answers: name a narrative ONLY when the flow truly has that
    // sign (inflow>0 / outflow<0). Distinct vocabulary ("заходять/виходять гроші")
    // avoids colliding with family-phase words ("набирають/роздають") → fewer wrong answers.
    let inflow_v = if hottest.1 > 0.0 && !hottest.0.is_empty() { json!(hottest.0) } else { Value::Null };
    let outflow_v = if coldest.1 < 0.0 && !coldest.0.is_empty() { json!(coldest.0) } else { Value::Null };

    // pre-filtered archetype lists so "who distributes/accumulates X" is a lookup
    let arch = fams["archetypes"].as_array().cloned().unwrap_or_default();
    let arch_with = |kw: &str| -> Vec<Value> {
        arch.iter().filter(|a| a["label"].as_str().map(|l| l.contains(kw)).unwrap_or(false) && a["count"].as_u64().unwrap_or(0) > 0).cloned().collect()
    };

    // Interpretive lens distilled from Michael Lewis, "Liar's Poker" (Salomon Bros
    // bond desk, 1980s). Principles for READING whale flow — shipped as data so the
    // model still answers only from the JSON. Each maps a book lesson → on-chain.
    // The bluff lesson ties directly to our event-study finding: heavy whale
    // accumulation precedes price DECLINE (the visible bet is often the trap).
    let principles = json!([
        {"урок": "Хто не бачить лоха за столом — сам лох", "застосування": "У кожній угоді є проінформована й непроінформована сторона. Роздріб, що купує на піку роздачі китів — і є лох. Завжди питай: ХТО контрагент цієї угоди?"},
        {"урок": "Інформаційна асиметрія — головна перевага", "застосування": "Salomon заробляв на клієнтах, що гірше знали ринок. Ти не переможеш китів інформацією; тому СЛІДКУЙ за ними — on-chain дає їхні ходи з затримкою, але дає."},
        {"урок": "Став лише коли шанси явно за тебе (дисципліна Мерівезера)", "застосування": "Не реагуй на кожен рух. Чекай сигналів високої конвікції: сильний узгоджений нетто-потік і збіг фаз багатьох родин, а не поодинокий сплеск."},
        {"урок": "Видима ставка часто блеф (суть гри 'покер брехунів')", "застосування": "Явне 'накопичення' китів може бути пасткою чи wash-трейдингом. Подієва аналітика системи показала: масове накопичення китів ПЕРЕДУЄ падінню ціни — сигнал буває інвертований. Не приймай накопичення за бичачий сигнал автоматично."},
        {"урок": "Наративи виробляються, а не виникають самі", "застосування": "Salomon створив ринок іпотечних облігацій, якого ніхто не розумів. Крипто-наративи (memecoin/RWA/DePIN) — це виготовлений попит; ротація капіталу між ними = цикли хайпу, а не фундаментал."},
        {"урок": "Ринком рухають жадібність і стадність, не логіка", "застосування": "Пік уваги й FOMO роздрібу — типовий момент, коли розумні гроші роздають. Масова фаза 'роздають' на тлі ажіотажу — тривожний, а не бичачий сигнал."},
        {"урок": "Лівермор: великі гроші — у терпінні, не в метушні; не борись зі стрічкою", "застосування": "Тренд нетто-потоку важливіший за одну угоду. Не торгуй проти стійкого напряму китів і не 'лови дно' — сиди в позиції, поки потік не розвернувся у даних."},
        {"урок": "Ґрем: ринок — маніакальний партнер (Mr. Market); ціна ≠ вартість; тримай запас міцності", "застосування": "Не плутай ажіотаж у наративі з цінністю. Вимагай підтвердження кількома родинами й кількома місяцями ротації, перш ніж діяти на сигналі."},
        {"урок": "Маккей: натовп божеволіє гуртом, а тверезіє поодинці", "застосування": "Пік узгодженого FOMO у наративі — пізня стадія циклу. Різкий приплив у наратив, куди щойно кинувся натовп, — сигнал обережності, а не входу."}
    ]);

    // most common non-null dominant sector across enriched families
    let mut sec_count: HashMap<String, usize> = HashMap::new();
    for f in &enriched {
        if let Some(sc) = f["dominantSector"].as_str() {
            *sec_count.entry(sc.to_string()).or_default() += 1;
        }
    }
    let dom_sector = sec_count.into_iter().max_by_key(|(_, n)| *n).map(|(k, _)| k);

    json!({
        "всьогоРодин": fams["familiesCount"],
        "збагаченоРодинДаними": enriched.len(),
        "фазиРодин": {"набирають": acc, "роздають": dist, "нейтральні": neu, "висновок": bigger},
        "домінантнийСекторРодин": dom_sector,
        "монетиЩоНабирають": coins_acc,
        "монетиЩоРоздають": coins_dist,
        "найбільшаРодина": largest,
        "архетипи": fams["archetypes"],
        "архетипиЩоРоздають": arch_with("роздає"),
        "архетипиЩоНабирають": arch_with("набирає"),
        "топРодини": top,
        "ротаціяНаративів": Value::Object(rotation),
        "наративКудиЗаходятьГрошіКитів": inflow_v,
        "наративЗвідкиВиходятьГрошіКитів": outflow_v,
        "принципиТоргівлі(лінзаЛьюіс)": principles,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Мультимовна grounded-гілка над базою знань `dataset/nextaction/kb-qa.jsonl`
// (генерується біном `kbqa`; факти детерміновані з даних груп, перекладені на 5 мов).
// Спрацьовує ЛИШЕ на чіткі питання про групи/емісію/контроль/прогноз/обʼєм — інакше
// повертає None і питання йде звичайним українським конвеєром нижче. Відповідає МОВОЮ ЗАПИТУ.
// ─────────────────────────────────────────────────────────────────────────────
const KB_LANGS: [&str; 5] = ["uk", "ru", "fr", "en", "de"];

fn kb_detect_lang(s: &str) -> &'static str {
    let has = |c: char| s.contains(c);
    if s.chars().any(|c| ('\u{0400}'..='\u{04FF}').contains(&c)) {
        if has('і') || has('ї') || has('є') || has('ґ') || has('І') || has('Ї') || has('Є') { return "uk"; }
        if has('ы') || has('э') || has('ъ') || has('ё') { return "ru"; } // ру-ексклюзивні літери
        // без розрізняльних літер — за словами; застосунок україномовний, тож дефолт uk
        let l = s.to_lowercase();
        if ["что", "если", "это", "сколько", " как ", " или ", "какой", "группа капитал", "имеет"].iter().any(|w| l.contains(w)) { return "ru"; }
        return "uk";
    }
    if has('ä') || has('ö') || has('ü') || has('ß') || has('Ä') || has('Ö') || has('Ü') { return "de"; }
    if has('à') || has('â') || has('é') || has('è') || has('ê') || has('ç') || has('ù') || has('î') || has('ï') || has('ô') { return "fr"; }
    let l = format!(" {} ", s.to_lowercase());
    let de_w = [" der ", " die ", " das ", " und ", " wie ", " viel ", " gruppe ", " welche ", " welchen ", " kontrolliert ", " hält ", " kapital ", " wer "];
    let fr_w = [" le ", " la ", " les ", " du ", " des ", " quelle ", " quel ", " groupe ", " contrôle ", " combien ", " émission ", " qui "];
    let dsc = de_w.iter().filter(|w| l.contains(*w)).count();
    let fsc = fr_w.iter().filter(|w| l.contains(*w)).count();
    if dsc > 0 && dsc >= fsc { return "de"; }
    if fsc > 0 { return "fr"; }
    "en"
}

fn kb_lang_name(l: &str) -> &'static str {
    match l { "uk" => "Ukrainian", "ru" => "Russian", "fr" => "French", "de" => "German", _ => "English" }
}
fn kb_lidx(l: &str) -> usize { KB_LANGS.iter().position(|&x| x == l).unwrap_or(3) }

fn kb_toks(s: &str) -> HashSet<String> {
    s.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| w.len() >= 2).map(|w| w.to_string()).collect()
}
fn kb_is_group_key(k: &str) -> bool {
    k == "a" || k == "b" || (k.len() == 2 && k.starts_with('w') && k.as_bytes()[1].is_ascii_digit())
}
fn kb_strong_keys(s: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    for raw in s.split(|c: char| c.is_whitespace() || matches!(c, ',' | '.' | '?' | '!' | ';' | ':' | '(' | ')' | '"' | '\'' | '@')) {
        let t = raw.trim();
        if t.is_empty() { continue; }
        let is_group = (t.len() == 1 && (t == "A" || t == "B")) || (t.len() == 2 && t.starts_with('W') && t.as_bytes()[1].is_ascii_digit());
        let is_ticker = t.len() >= 2 && t.len() <= 10 && t.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) && t.chars().any(|c| c.is_ascii_uppercase());
        if is_group || is_ticker { out.insert(t.to_lowercase()); }
    }
    out
}
/// Множина «великих» чисел (≥4 цифр) у тексті, з видаленими роздільниками тисяч
/// ('.'/','/'\''/nbsp), щоб порівнювати $-суми незалежно від локального форматування.
fn kb_nums(s: &str) -> HashSet<String> {
    let cleaned: String = s.chars().filter(|c| !matches!(c, '.' | ',' | '\'' | '\u{00a0}' | '\u{202f}')).collect();
    let mut out = HashSet::new();
    let mut cur = String::new();
    for c in cleaned.chars() {
        if c.is_ascii_digit() { cur.push(c); }
        else { if cur.len() >= 4 { out.insert(std::mem::take(&mut cur)); } cur.clear(); }
    }
    if cur.len() >= 4 { out.insert(cur); }
    out
}

fn kb_intent_dim(q: &str) -> Option<&'static str> {
    let l = q.to_lowercase();
    let hit = |ws: &[&str]| ws.iter().any(|w| l.contains(w));
    // alarm/risk/global — ПЕРШИМИ (бо «група капіталу» містить «капітал» і не має плутатись із volume)
    if hit(&["тривог", "alarm", "alert", "тревог", "alarme", "alerte", "warnung", "під наглядом зараз", "ось-ось розда", "ось-ось скин", "who is about to"]) { return Some("alarm"); }
    if hit(&["сценар", "scenario", "стрес", "stress", "supply-shock", "supply shock", "якщо розда", "якщо роздасть", "розпродасть", "if.*distribut", "was passiert wenn", "que se passe", "supply-shock", "шок пропозиц"]) { return Some("scenario"); }
    if hit(&["ризик", "riskant", "risque", "risk", "риск", "небезпечн", "найнебезпеч"]) { return Some("risk"); }
    if hit(&["скільки груп", "сукупн", "вся мереж", "всієї мереж", "домінують мереж", "загалом більш", "total capital", "how many group", "dominate the network", "gesamtkapital", "combien de groupe", "wie viele gruppe", "сколько групп"]) { return Some("global"); }
    if hit(&["kapital", "capital", "капітал", "капитал", "volume", "обсяг", "объём", "объем", "insgesamt", "montant"]) { return Some("volume"); }
    if hit(&["probab", "вероятн", "ймовір", "prognos", "prévision", "verteil", "роздач", "роздава", "распрода", "distribut"]) { return Some("prediction"); }
    if hit(&["emiss", "эмисс", "émission", "emission", "supply", "постач"]) { return Some("emission"); }
    if hit(&["coordina", "координ", "synchron", "узгодж", "согласован"]) { return Some("coordination"); }
    if hit(&["connect", "зв'яз", "звяз", "связ", "holder", "холдер", "gemeinsam"]) { return Some("connections"); }
    None
}

/// Канонічні id груп, згадані в питанні: "a","b","cap:15","w1"… (групи тепер CAP:N + A/B).
/// Розпізнає: standalone A/B, CAP:N / CAP N / capN, «група/group/gruppe N».
fn kb_group_ids(q: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    for raw in q.split(|c: char| !c.is_ascii_alphanumeric()) {
        if raw == "A" || raw == "B" || raw == "NET" { out.insert(raw.to_lowercase()); }
        if raw.len() == 2 && raw.starts_with('W') && raw.as_bytes()[1].is_ascii_digit() { out.insert(raw.to_lowercase()); }
        if raw.len() == 3 && raw.starts_with("HG") && raw.as_bytes()[2].is_ascii_digit() { out.insert(raw.to_lowercase()); } // HG2..HG9 сателіти
    }
    let lo = q.to_lowercase();
    // слово «мережа/network» -> група NET
    if ["мереж", "network", "netzwerk", "réseau", "reseau", "сеть капит"].iter().any(|w| lo.contains(w)) { out.insert("net".into()); }
    let b = lo.as_bytes();
    let grab_num = |mut j: usize| -> Option<(usize, usize)> {
        while j < b.len() && (b[j] == b':' || b[j] == b' ') { j += 1; }
        let s = j;
        while j < b.len() && b[j].is_ascii_digit() { j += 1; }
        if j > s { Some((s, j)) } else { None }
    };
    // CAP:N / CAP N / capN
    let mut from = 0;
    while let Some(p) = lo[from..].find("cap") {
        let i = from + p;
        if let Some((s, e)) = grab_num(i + 3) { out.insert(format!("cap:{}", &lo[s..e])); }
        from = i + 3;
    }
    // «група/group/gruppe/groupe/группа N» -> cap:N
    for gw in ["груп", "group", "gruppe", "groupe", "группа"] {
        let mut from = 0;
        while let Some(p) = lo[from..].find(gw) {
            let idx = from + p + gw.len();
            if let Some((s, e)) = grab_num(idx) { out.insert(format!("cap:{}", &lo[s..e])); }
            from = idx;
        }
    }
    // «сателіт/satellite N» -> hgN
    for gw in ["сателіт", "satellite", "сателлит", "satelliten"] {
        let mut from = 0;
        while let Some(p) = lo[from..].find(gw) {
            let idx = from + p + gw.len();
            if let Some((s, e)) = grab_num(idx) { out.insert(format!("hg{}", &lo[s..e])); }
            from = idx;
        }
    }
    out
}

/// Переклад готової відповіді мовою `lang` зі збереженням чисел (число-гард). None → не чіпати оригінал.
async fn kb_translate(base: &str, model: &str, key: &str, text: &str, lang: &str) -> Option<String> {
    let ln = kb_lang_name(lang);
    let system = format!(
        "You are a professional translator for a crypto on-chain analytics app. Translate the user's Ukrainian text into {ln}. \
GLOSSARY — translate by MEANING, never literally: 'кит'/'кити' = a crypto WHALE / large holder (EN whale, FR baleine, DE Wal, RU кит) — NEVER 'cat'/'kitty'/'kiki'; \
'наратив' = market narrative/sector; 'набирають'/'накопичують' = accumulating (buying); 'роздають'/'скидають' = distributing (selling); \
'притік' = inflow, 'відтік' = outflow; 'родина' = wallet family/cluster; 'гаманець' = wallet. \
Keep sector names (DePIN, DeFi, RWA, memecoin) and ALL numbers, %, $ amounts, tickers and 0x addresses EXACTLY — do NOT reformat or change any figure. \
Output ONLY the translation, nothing else."
    );
    let body = json!({"model": model, "max_tokens": 700, "temperature": 0.0,
        "messages": [{"role": "system", "content": system}, {"role": "user", "content": text}]});
    let client = reqwest::Client::new();
    let url = format!("{}/chat/completions", base.trim_end_matches('/'));
    let resp = client.post(&url).header("authorization", format!("Bearer {key}")).header("content-type", "application/json").json(&body).send().await.ok()?;
    if !resp.status().is_success() { return None; }
    let v: Value = resp.json().await.ok()?;
    let t = v["choices"][0]["message"]["content"].as_str().unwrap_or("").trim().to_string();
    if t.is_empty() { return None; }
    let src = kb_nums(text);
    if kb_nums(&t).iter().any(|n| !src.contains(n)) { return None; } // переклад спотворив число
    Some(t)
}

/// Пробує відповісти з grounded-KB мовою запиту. None → не наша тема, іде звичайний конвеєр.
async fn try_kb_answer(s: &Ctx, question: &str, base: &str, model: &str, key: &str) -> Option<axum::response::Response> {
    let ql = question.to_lowercase();
    let qk = kb_strong_keys(question);
    let gids = kb_group_ids(question);            // {"a","b","cap:15",…}
    let group_ref = !gids.is_empty();
    // тікер = ALLCAPS-ключ, що не є group-літерою, не слово "cap" і не згаданий group-id
    let has_ticker = qk.iter().any(|k| !kb_is_group_key(k) && k != "cap" && !gids.contains(k.as_str()));
    let has_ctrl_emis = ["контрол", "control", "kontroll", "contrôl", "emiss", "емісі", "эмисс", "émission", "хто контр", "who control", "wer kontroll", "qui contrôl"]
        .iter().any(|k| ql.contains(k));
    let has_group_word = ["груп", "group", "gruppe", "groupe", "группа", "мереж", "network", "netzwerk", "сателіт", "satellite"].iter().any(|k| ql.contains(k));
    let idim = kb_intent_dim(question);
    // global-питання й risk-ПРО-ГРУПУ маршрутизуються самі (risk про монети лишається legacy-волатильності)
    let global_dim = idim == Some("global") || idim == Some("alarm") || idim == Some("scenario") || (idim == Some("risk") && (group_ref || has_group_word));
    // KB-намір: група (A/B/NET/HG…) АБО global/risk-про-групу АБО (контроль/емісія + тікер) АБО (вимір + тікер).
    let kb_intent = group_ref || global_dim || (has_ctrl_emis && has_ticker) || (idim.is_some() && has_ticker);
    if !kb_intent { return None; }

    let lang = kb_detect_lang(question);
    let li = kb_lidx(lang);

    let text = std::fs::read_to_string(s.dataset.root().join("nextaction/kb-qa.jsonl")).ok()?;
    struct R { dim: String, q_all: String, ans: [String; 5], meta: String, gid: String, is_agg: bool, is_wallet: bool }
    let mut recs: Vec<R> = Vec::new();
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        let q_all = KB_LANGS.iter().filter_map(|l| v["q"][l].as_str()).collect::<Vec<_>>().join(" ");
        let mut ans: [String; 5] = std::array::from_fn(|_| String::new());
        for (i, l) in KB_LANGS.iter().enumerate() { ans[i] = v["a"][l].as_str().unwrap_or("").to_string(); }
        let mobj = v["meta"].as_object();
        let meta = mobj.map(|m| m.values().filter_map(|x| x.as_str().map(|z| z.to_string()).or(x.as_f64().map(|f| format!("{f}")))).collect::<Vec<_>>().join(" ")).unwrap_or_default();
        let has_wallet = mobj.map(|m| m.contains_key("wallet")).unwrap_or(false);
        let has_group = mobj.map(|m| m.contains_key("group")).unwrap_or(false);
        let gid = mobj.and_then(|m| m.get("group")).and_then(|x| x.as_str()).map(|s| s.to_lowercase()).unwrap_or_default();
        recs.push(R { dim: v["dim"].as_str().unwrap_or("").to_string(), q_all, ans, meta, gid, is_agg: has_group && !has_wallet, is_wallet: has_wallet });
    }
    if recs.is_empty() { return None; }

    let qt = kb_toks(question);
    let q_addr = ql.contains("0x");
    let group_scope = group_ref && !has_ticker && !q_addr;
    let mut scored: Vec<(f64, &R)> = recs.iter().map(|r| {
        let rt = kb_toks(&r.q_all); let mt = kb_toks(&r.meta); let mk = kb_strong_keys(&r.meta);
        let mut sc = 0.0;
        for w in &qt { if mt.contains(w) { sc += 3.0; } else if rt.contains(w) { sc += 1.0; } }
        for w in &qk { if mk.contains(w) { sc += 5.0; } }
        if !r.gid.is_empty() && gids.contains(&r.gid) { sc += 6.0; }  // прямий збіг group-id (вкл. CAP:N)
        if idim == Some(r.dim.as_str()) { sc += 3.0; }
        if group_scope && r.is_agg { sc += 4.0; }
        if group_scope && r.is_wallet && sc > 0.0 { sc -= 2.5; }
        (sc, r)
    }).collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    let top: Vec<&R> = scored.iter().filter(|(sc, _)| *sc > 0.0).take(8).map(|(_, r)| *r).collect();
    if top.is_empty() { return None; }

    let facts = top.iter().enumerate().map(|(i, r)| {
        let a = if !r.ans[li].is_empty() { &r.ans[li] } else if !r.ans[0].is_empty() { &r.ans[0] } else { &r.ans[3] };
        format!("{}. {}", i + 1, a)
    }).collect::<Vec<_>>().join("\n");

    let ln = kb_lang_name(lang);
    let system = format!(
        "You are a professional crypto on-chain analytics assistant for the Wallet Radar wallet-surveillance app. \
Answer the user's QUESTION using ONLY the FACTS provided (deterministic on-chain facts about capital groups under surveillance). \
If the facts do not cover the question, say so honestly — NEVER invent numbers, tickers or addresses. \
Write your ENTIRE answer in {ln} and nothing else. Be concise and professional (2-5 sentences). \
Keep all numbers, %, $ amounts, tickers and 0x addresses EXACTLY."
    );
    let user = format!("FACTS ({ln}):\n{facts}\n\nQUESTION: {question}\n\nANSWER (in {ln}):");
    let body = json!({
        "model": model, "max_tokens": 600, "temperature": 0.0,
        "messages": [{"role": "system", "content": system}, {"role": "user", "content": user}]
    });
    let client = reqwest::Client::new();
    let url = format!("{}/chat/completions", base.trim_end_matches('/'));
    let resp = client.post(&url).header("authorization", format!("Bearer {key}")).header("content-type", "application/json").json(&body).send().await.ok()?;
    let status = resp.status();
    let v: Value = resp.json().await.ok()?;
    if !status.is_success() || v.get("error").is_some() { return None; }
    let mut answer = v["choices"][0]["message"]["content"].as_str().unwrap_or("").trim().to_string();
    if answer.is_empty() { return None; }
    // Гарантія точності чисел: якщо модель ввела $-суму/велике число, якого НЕМА у фактах
    // (переформатувала чи вигадала) — повертаємо найкращий факт дослівно (він точний і потрібною мовою).
    let fact_nums = kb_nums(&facts);
    let mut source = "kb";
    if kb_nums(&answer).iter().any(|n| !fact_nums.contains(n)) {
        answer = top[0].ans[li].clone();
        if answer.is_empty() { answer = top[0].ans[3].clone(); }
        source = "kb-verbatim";
    }
    // Якщо KB чесно сказав «фактів нема» — це НЕ фінальна відповідь. Пропускаємо
    // питання далі (інструменти мають доступ до всіх 73 груп, а KB — лише до частини).
    let al = answer.to_lowercase();
    let kb_declined = [
        "не можу надати", "факти не включають", "немає інформації", "немає даних",
        "не містять інформації", "недостатньо даних", "не покривають",
        "не надано", "не вказано", "відсутні дані", "бракує даних", "не знайдено",
        "нема даних", "не маю даних", "не містять даних", "не надають", "no data",
        "в даних немає", "даних немає", "фактів про", "інформації про", "не згадується",
        "do not cover", "not covered", "no information", "cannot provide",
    ]
    .iter()
    .any(|m| al.contains(m));
    if kb_declined {
        return None;
    }

    Some(Json(json!({"configured": true, "answer": answer, "model": v["model"], "source": source, "lang": lang})).into_response())
}

pub async fn ask_route(State(s): State<Ctx>, Query(q): Query<AskQuery>) -> impl IntoResponse {
    let question = q.q.unwrap_or_default().trim().to_string();
    if question.is_empty() {
        return Json(json!({"error": "порожнє питання"})).into_response();
    }
    // Питання про ТЕРМІН («що таке X», «поясни X») — детермінована відповідь із глосарію.
    // Стоїть ПЕРЕД усіма гілками і НЕ потребує моделі: раніше такі питання підхоплювали
    // хибний якір-підказку про наративи й відповідали не на те, що спитали.
    {
        let dsdir = std::env::var("DATASET_DIR")
            .unwrap_or_else(|_| "/opt/wallet-tracker/dataset".to_string());
        // 1) точне визначення, якщо термін відомий
        if let Some(v) = crate::glossary::try_answer_found(&question) {
            return Json(v).into_response();
        }
        // 2) якщо терміна нема — спершу пробуємо базу знань про продукт (нижче),
        //    і лише якщо й вона мовчить, повертаємось до відмови глосарію
        if crate::product_kb::find_topic(&question).is_none() {
            if let Some(v) = crate::glossary::try_answer(&question, &dsdir) {
                return Json(v).into_response();
            }
        }
    }

    // Питання ПРО САМУ СИСТЕМУ (що це, як працює, що знайдено, чого не вміє) —
    // теж детерміновано: факти про власний продукт вигадувати не можна.
    if let Some(v) = crate::product_kb::try_answer(&question) {
        return Json(v).into_response();
    }

    let key = std::env::var("LLM_API_KEY").unwrap_or_default();
    if key.is_empty() {
        return Json(json!({
            "configured": false,
            "answer": "AI-асистент не налаштований. Підніми локальну модель (Ollama) або додай LLM_API_KEY у /opt/wallet-tracker/.env і перезапусти wallet-radar-rust."
        }))
        .into_response();
    }
    let base = std::env::var("LLM_BASE_URL").unwrap_or_else(|_| "https://api.groq.com/openai/v1".into());
    let model = std::env::var("LLM_MODEL").unwrap_or_else(|_| "llama-3.3-70b-versatile".into());

    // Мультимовна grounded-гілка над базою знань груп (kb-qa.jsonl). Спрацьовує лише на
    // чіткі питання про групи/емісію/контроль/прогноз/обʼєм і відповідає мовою запиту;
    // усе інше йде звичайним українським конвеєром нижче — його поведінка не змінюється.
    // KB зберігає власні факти, згенеровані окремо, і вони можуть розійтися з
    // датасетом (виявлено: NET — 3238 гаманців у KB проти 6864 у capital-groups).
    // Для ЧИСЛОВИХ питань про групи джерело правди одне — датасет через інструменти.
    let numeric_group_q = {
        let ql = question.to_lowercase();
        (ql.contains("скільки") || ql.contains("розмір") || ql.contains("капітал")
            || ql.contains("адрес") || ql.contains("гаманц"))
            && crate::tools::mentions_group(&question)
    };
    if !numeric_group_q {
    if let Some(resp) = try_kb_answer(&s, &question, &base, &model, &key).await {
        return resp;
    }
    }

    let ctx = build_context(&s);

    // Deterministic intent routing for capital-flow questions — the top truth-risk on a
    // small model. Instead of letting it choose among look-alike fields (phase vs
    // archetype vs rotation), we detect flow intent in Rust, compute the exact answer,
    // and hand it over as a fact for the model to verbalize.
    let ql = question.to_lowercase();
    let acc_n = ctx["фазиРодин"]["набирають"].as_u64().unwrap_or(0);
    let dist_n = ctx["фазиРодин"]["роздають"].as_u64().unwrap_or(0);
    let fam_total = ctx["всьогоРодин"].as_u64().unwrap_or(0);
    let enriched_n = ctx["збагаченоРодинДаними"].as_u64().unwrap_or(0);
    let inflow = ctx["наративКудиЗаходятьГрошіКитів"].as_str();
    let outflow = ctx["наративЗвідкиВиходятьГрошіКитів"].as_str();

    let coins_intent = ["монет", "токен", " coin"].iter().any(|k| ql.contains(k));
    let named_all: Vec<&str> = ["depin", "memecoin", "defi", "rwa"].into_iter().filter(|n| ql.contains(n)).collect();
    let named = named_all.first().copied();
    let nodata_q = ["капіталіз", "халвінг", "market cap", "ринкова кап"]
        .iter().any(|k| ql.contains(k));
    // питання про ЦІНУ/ГРАФІК/технічний аналіз — цих даних система не має → чесно визнати межу + перекинути на потоки китів
    let chart_price_q = ["графік", "чарт", "свічк", "технічн", "підтрим", "опір", "ціни", "ціна ", "курс ", "коштуват", "почім", "вартіст", "спрогнозуй", "прогноз", "куди піде", "росте чи пад", "падає чи рост", "рух ціни"]
        .iter().any(|k| ql.contains(k));
    // базове питання напряму фаз ('купують чи продають', 'набирають чи роздають') — без цієї
    // гілки воно падало у фолбек-без-хінта й модель розсипалась дампом усіх полів+принципів.
    let dir_q = (ql.contains(" чи ") || ql.contains("більше"))
        && ["купу", "набира", "накопич"].iter().any(|k| ql.contains(k))
        && ["прода", "розда", "скида"].iter().any(|k| ql.contains(k))
        && !coins_intent;
    let count_q = ql.contains("скільки") && ql.contains("родин");
    let contrarian_q = ql.contains("накопич")
        && ["купувати", "сигнал", "варто", "довір", "слідом", "заходити", "бичач"].iter().any(|k| ql.contains(k));
    let comp_word = named_all.len() >= 2
        && (["сильніше", "найсильніше", "дужче", "більше", "найбільше"].iter().any(|k| ql.contains(k)) || ql.contains(" чи "));
    let who_sector = named.is_some()
        && ["хто", "родин", "архетип", "які групи"].iter().any(|k| ql.contains(k))
        && ["накопич", "набира", "розда", "прода", "тримают"].iter().any(|k| ql.contains(k))
        && !["заход", "виход", "течуть", "притік", "відтік", "куди", "звідки"].iter().any(|k| ql.contains(k));
    let outflow_intent = ["виход", "відтік", "втрачає", "звідки", "покида", "з якого"].iter().any(|k| ql.contains(k));
    let inflow_intent = ["заход", "течуть", "притік", "гарячий", "обертів", "куди йдут", "куди вклад", "найпопуляр", "ротац"]
        .iter().any(|k| ql.contains(k));
    // ticker named in the question (uppercase ASCII, 2-6 chars) for membership questions
    let q_ticker = question.split(|c: char| !c.is_ascii_alphanumeric())
        .find(|w| (2..=6).contains(&w.len()) && w.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) && w.chars().any(|c| c.is_ascii_uppercase()))
        .map(|s| s.to_uppercase());
    let membership_q = q_ticker.is_some() && (ql.contains("купу") || ql.contains("тримают") || ql.contains("скида"));
    let neu_n = ctx["фазиРодин"]["нейтральні"].as_u64().unwrap_or(0);
    let acc_syms_top: Vec<String> = ctx["монетиЩоНабирають"].as_array()
        .map(|a| a.iter().filter_map(|x| x["токен"].as_str()).take(5).map(|s| s.to_string()).collect())
        .unwrap_or_default();
    let dist_syms_top: Vec<String> = ctx["монетиЩоРоздають"].as_array()
        .map(|a| a.iter().filter_map(|x| x["токен"].as_str()).take(5).map(|s| s.to_string()).collect())
        .unwrap_or_default();
    let help_q = ["привіт", "допоможи", "що ти вмієш", "що ти можеш", "що вмієш", "чим допоможеш", "як користув", "хто ти", "твої можливост", "що вміє"]
        .iter().any(|k| ql.contains(k));
    let advice_q = ["порадь", "порада", "підсумуй", "підсумок", "огляд", "загальн", "що робити", "як діяти", "стратег", "тренд", "налаштован"]
        .iter().any(|k| ql.contains(k));
    let top1_q = ql.contains("монет")
        && ["головн", "№1", "номер один", "топ-1", "найголовніш", "перша монет", "найважлив"].iter().any(|k| ql.contains(k));
    let portfolio_q = ["портфель", "портфоліо", "що купити", "у що інвест", "які альткоїн", "порадь монет", "підбери монет", "склади портфель", "збери портфель"]
        .iter().any(|k| ql.contains(k));
    let compound_q = (ql.contains("купу") || ql.contains("накопич") || ql.contains("набира"))
        && (ql.contains("куди") || ql.contains("наратив") || ql.contains("сектор"))
        && (ql.contains(" і ") || ql.contains(" та "));
    let month_q = ql.contains("місяц");
    let domsec_q = ql.contains("сектор") && (ql.contains("домін") || ql.contains("переваж") || ql.contains("найпоширен") || ql.contains("найбільше родин"));
    let types_q = (ql.contains("тип") && ql.contains("родин"))
        || (ql.contains("архетип") && ["які", "перелік", "перелічи", "опиши", "назви", "бувают", "розкажи про"].iter().any(|k| ql.contains(k)));
    let share_q = ql.contains("родин") && (ql.contains("частк") || ql.contains("відсот") || ql.contains("%") || ql.contains("процент"));

    // Deterministic intent routing (precedence matters). Rust computes the exact answer;
    // the model only verbalizes it. Order: no-data → largest-size → archetype-count →
    // count → contrarian → comparative → who-accumulates-sector → coins → membership →
    // named-narrative flow → both-directions → outflow → inflow.
    let vol_q = ["волатильн", "великий рух", "різкий рух", "сильний рух", "рвоне", "рвонут", "стрибне", "стрельне", "феєрверк", "straddle", "рознесе", "прострелит"].iter().any(|k| ql.contains(k));
    let whatis_q = ["що таке кит", "хто такі кит", "хто такий кит", "що означає кит", "поясни хто", "розкажи хто так"].iter().any(|k| ql.contains(k));
    let sentiment_q = ["ведмеж", "бичач", "настрій ринк", "настрої ринк", "бик чи ведм", "бул чи ведм", "ринок настро"].iter().any(|k| ql.contains(k));
    let risk_q = ["ризик", "найризик", "небезпечн", "тонко трим", "тонка база", "крихк", "обвал", "падатим", "падуть", "просяд"].iter().any(|k| ql.contains(k)) && !contrarian_q;
    let mut hint = String::new();
    let mut unmatched = false;   // жоден інтент не підійшов → підемо в режим інструментів
    if help_q {
        hint = "\n\nПРЯМА ПІДКАЗКА-ФАКТ: це вітання або питання про можливості. Відповідай РІВНО: 'Я AI-аналітик Wallet Radar. Можу сказати: що кити купують чи продають, які монети накопичують або скидають, куди течуть гроші по наративах, які родини й архетипи активні, і застерегти щодо сигналів. Наприклад запитай: «які монети накопичують кити?» чи «куди заходять гроші китів?».'".into();
    } else if nodata_q && !coins_intent {
        hint = "\n\nПРЯМА ПІДКАЗКА-ФАКТ: питання про капіталізацію/подію монети — таких даних у системі НЕМА. Відповідай РІВНО: 'Я не прогнозую ціни і не маю ринкових даних — я показую лише потоки китів у наявних даних.' Нічого не додавай.".into();
    } else if chart_price_q && !coins_intent {
        let bs = if acc_n > dist_n { "більше НАБИРАЮТЬ (накопичують)" } else if dist_n > acc_n { "більше РОЗДАЮТЬ (продають)" } else { "нейтральні" };
        let inf = inflow.map(|n| format!("капітал заходить у наратив {n}")).unwrap_or_else(|| "чистого притоку в жоден наратив зараз нема".into());
        hint = format!("\n\nПРЯМА ПІДКАЗКА-ФАКТ: це питання про ЦІНУ/ГРАФІК/технічний аналіз — таких даних (цін, свічок, рівнів підтримки/опору) у системі НЕМА. Відповідай так: (1) чесно визнай межу — 'Я не показую цінові графіки й не прогнозую ціни; я показую лише потоки китів у наявних даних'; (2) поверни на суть, що знаєш: кити зараз {bs}, {inf}; (3) додай, що потоки китів часто ПЕРЕДУЮТЬ ціні, але це НЕ пряме передбачення ціни. КАТЕГОРИЧНО не вигадуй цін, рівнів підтримки/опору, трендів чи фігур графіка.");
    } else if vol_q {
        let vd = s.dataset.read_or("strategy-volatility.json", json!({"tokens": []}));
        let toks: Vec<String> = vd["tokens"].as_array().map(|a| a.iter().take(6)
            .map(|x| {
                let sym = x["symbol"].as_str().filter(|z| !z.is_empty()).or_else(|| x["token"].as_str()).unwrap_or("?");
                let pp = x["pBigMove"].as_f64().unwrap_or(0.0);
                format!("{} ({:.0}%)", sym, pp * 100.0)
            }).collect())
            .unwrap_or_default();
        let mauc = vd["model"]["testAuc"].as_f64().unwrap_or(0.6);
        if toks.is_empty() {
            hint = "\n\nПРЯМА ПІДКАЗКА-ФАКТ: питання про волатильність/великі рухи. Зараз модель не виділяє жодного токена з підвищеною ймовірністю великого руху. Відповідай РІВНО: 'Наразі немає токенів із вираженим сигналом підвищеної волатильності серед тих, де кити тяжко набирають.'".into();
        } else {
            hint = format!("\n\nПРЯМА ПІДКАЗКА-ФАКТ: візьми за ТОЧНУ ОСНОВУ й РОЗГОРНИ конкретними числами. Модель волатильності (whale-flow, OOS AUC ~{:.2}) флагує токени, де кити ЗАРАЗ тяжко набирають USD → підвищена ймовірність ВЕЛИКОГО руху близько ±25%% за ~30 днів. Топ за ймовірністю: {}. ОБОВ'ЯЗКОВО познач: це прогноз ВЕЛИЧИНИ руху, а НЕ напряму — модель не каже вгору чи вниз (торгується як straddle/очікування різкого руху). Категорично не вигадуй напрям, цільові ціни чи рівні підтримки/опору.", mauc, toks.join(", "));
        }
    } else if whatis_q {
        hint = "\n\nПРЯМА ПІДКАЗКА-ФАКТ: питання-визначення (хто такі кити). Відповідай РІВНО: 'Кит — гаманець із великим капіталом, чиї рухи система відстежує ончейн (Etherscan V2), бо вони часто передують ринковим змінам. Я показую, що кити купують і продають, куди течуть їхні гроші по наративах, і застерігаю щодо сигналів.' Коротко, без вигаданих деталей.".into();
    } else if sentiment_q {
        let lean = if acc_n > dist_n {
            format!("більше НАБИРАЮТЬ (накопичують) — {acc_n} родин проти {dist_n}, тобто радше БИЧАЧИЙ ухил серед китів")
        } else if dist_n > acc_n {
            format!("більше РОЗДАЮТЬ (продають) — {dist_n} проти {acc_n}, тобто радше ВЕДМЕЖИЙ ухил")
        } else { "приблизно нейтрально".to_string() };
        let inf = inflow.map(|n| format!(", капітал заходить у наратив {n}")).unwrap_or_default();
        hint = format!("\n\nПРЯМА ПІДКАЗКА-ФАКТ: візьми за ТОЧНУ ОСНОВУ й РОЗГОРНИ. Настрій за ОНЧЕЙН-потоками китів: {lean}{inf}. ВАЖЛИВО познач: це настрій за рухами китів, а НЕ прогноз ціни — я не маю ринкових цін чи індексу страху/жадібності.");
    } else if risk_q {
        let vd = s.dataset.read_or("strategy-volatility.json", json!({"tokens": []}));
        let toks: Vec<String> = vd["tokens"].as_array().map(|a| a.iter().take(5)
            .map(|x| {
                let sym = x["symbol"].as_str().filter(|z| !z.is_empty()).or_else(|| x["token"].as_str()).unwrap_or("?");
                format!("{} ({:.0}%)", sym, x["pBigMove"].as_f64().unwrap_or(0.0) * 100.0)
            }).collect())
            .unwrap_or_default();
        if toks.is_empty() {
            hint = "\n\nПРЯМА ПІДКАЗКА-ФАКТ: питання про ризик/волатильність. Зараз модель не виділяє токенів із підвищеним ризиком великого руху серед тих, де кити тяжко набирають.".into();
        } else {
            hint = format!("\n\nПРЯМА ПІДКАЗКА-ФАКТ: візьми за ТОЧНУ ОСНОВУ. Найризикованіші за моделлю волатильності (висока ймовірність ВЕЛИКОГО ±руху за ~30д): {}. ОБОВ'ЯЗКОВО познач: ризик тут = великий рух у БУДЬ-ЯКИЙ бік (не обов'язково падіння); найкрихкіші — токени з ТОНКОЮ базою власників. Я НЕ прогнозую напрям — не стверджуй впевнено 'впаде' чи 'зросте'.", toks.join(", "));
        }
    } else if dir_q {
        let bs = if acc_n > dist_n { "більше НАБИРАЮТЬ (накопичують/купують)" } else if dist_n > acc_n { "більше РОЗДАЮТЬ (продають)" } else { "приблизно нейтральні" };
        hint = format!("\n\nПРЯМА ПІДКАЗКА-ФАКТ: почни з прямого висновку:'Кити зараз {bs}.' Підкріпи ТОЧНИМИ лічильниками: {acc_n} родин набирають, {dist_n} роздають, {neu_n} нейтральні (із {enriched_n} збагачених). 2-4 речення, БЕЗ переліку принципів лінзи й БЕЗ дампу топРодин/архетипів.");
    } else if portfolio_q {
        let coins = if acc_syms_top.is_empty() { "—".to_string() } else { acc_syms_top.join(", ") };
        let inf = ctx["наративКудиЗаходятьГрошіКитів"].as_str().map(|n| format!(" Капітал заходить у наратив {n}.")).unwrap_or_default();
        hint = format!("\n\nПРЯМА ПІДКАЗКА-ФАКТ: це прохання про портфель / що купити. Відповідай на основі даних: назви альткоїни, які кити зараз НАКОПИЧУЮТЬ: {coins}.{inf} ОБОВ'ЯЗКОВО закінчи дослівно: 'Це не фінансова порада — лише те, що накопичують кити; за нашими даними масове накопичення часто ПЕРЕДУЄ падінню ціни, тож рішення за тобою.'");
    } else if ql.contains("найбільш") && (ql.contains("скільки") || ql.contains("гаманц") || ql.contains("розмір")) {
        let sz = ctx["найбільшаРодина"]["розмірГаманців"].as_u64().unwrap_or(0);
        hint = format!("\n\nПРЯМА ПІДКАЗКА-ФАКТ: візьми за ТОЧНУ ОСНОВУ й РОЗГОРНИ її конкретними числами з JSON:'У найбільшій родині {sz} гаманців.'");
    } else if ql.contains("найбільш") && (ql.contains("обсяг") || ql.contains("оборот") || ql.contains("volume")) {
        let v = ctx["найбільшаРодина"]["обсягUSD"].as_f64().unwrap_or(0.0);
        let vs = if v >= 1e9 { format!("${:.1} млрд", v / 1e9) } else if v >= 1e6 { format!("${:.1} млн", v / 1e6) } else { format!("${:.0}", v) };
        hint = format!("\n\nПРЯМА ПІДКАЗКА-ФАКТ: візьми за ТОЧНУ ОСНОВУ й РОЗГОРНИ її конкретними числами з JSON:'Обсяг угод найбільшої родини — {vs}.'");
    } else if ql.contains("родин")
        && (ql.contains("найбільша ") || ql.contains("найбільшу ") || ql.contains("найбільшої "))
        && !ql.contains("наратив") && !ql.contains("сектор") {
        let f = &ctx["найбільшаРодина"];
        let phase = f["фаза"].as_str().unwrap_or("невідомо");
        let arch = f["архетип"].as_str().unwrap_or("—");
        let sec = f["домінантнийСектор"].as_str().unwrap_or("—");
        let tok = f["топТокенЕмісії"].as_str().unwrap_or("—");
        let sz = f["розмірГаманців"].as_u64().unwrap_or(0);
        hint = format!("\n\nПРЯМА ПІДКАЗКА-ФАКТ: візьми за ТОЧНУ ОСНОВУ й РОЗГОРНИ її конкретними числами з JSON:'Найбільша родина ({sz} гаманців) зараз {phase} (архетип: {arch}), домінантний сектор — {sec}, топ-токен емісії {tok}.'");
    } else if ql.contains("скільки") && ql.contains("архетип") {
        let n = ctx["архетипи"].as_array().map(|a| a.iter().filter(|x| x["count"].as_u64().unwrap_or(0) > 0).count()).unwrap_or(0);
        hint = format!("\n\nПРЯМА ПІДКАЗКА-ФАКТ: візьми за ТОЧНУ ОСНОВУ й РОЗГОРНИ її конкретними числами з JSON:'Виділено {n} архетипів китових родин.'");
    } else if types_q {
        let labels: Vec<String> = ctx["архетипи"].as_array()
            .map(|a| a.iter()
                .filter(|x| x["count"].as_u64().unwrap_or(0) > 0)
                .filter_map(|x| x["label"].as_str().map(|l| format!("{} ({})", l, x["count"].as_u64().unwrap_or(0))))
                .collect())
            .unwrap_or_default();
        if labels.is_empty() {
            hint = "\n\nПРЯМА ПІДКАЗКА-ФАКТ: візьми за ТОЧНУ ОСНОВУ й РОЗГОРНИ її конкретними числами з JSON:'Архетипів родин ще не виділено.'".into();
        } else {
            hint = format!("\n\nПРЯМА ПІДКАЗКА-ФАКТ: перелічи РОЗГОРНУТО архетипи (типи) китових родин із лічильниками й коротко поясни кожен (фаза+сектор): {}.", labels.join("; "));
        }
    } else if count_q {
        let (num, what) = if ql.contains("накопич") || ql.contains("набира") {
            (acc_n, "зараз накопичують (набирають)")
        } else if ql.contains("розда") || ql.contains("прода") {
            (dist_n, "зараз роздають (продають)")
        } else if ql.contains("нейтр") {
            (neu_n, "зараз нейтральні")
        } else if ql.contains("всього") || ql.contains("відстеж") || ql.contains("загал") {
            (fam_total, "усього відстежується")
        } else {
            (enriched_n, "збагачені даними")
        };
        hint = format!("\n\nПРЯМА ПІДКАЗКА-ФАКТ: почни відповідь ТОЧНИМ числом:'Зараз {num} родин {what}.' Число {num} має стояти на ПОЧАТКУ й бути незмінним. Можеш коротко додати контекст (загальна к-сть збагачених родин, співвідношення набирають/роздають), але НЕ приплітай конкретний сектор/наратив, якщо про нього не питали, і не заміняй {num} іншим числом.");
    } else if share_q {
        let total = enriched_n.max(1);
        let (num, what) = if ql.contains("розда") || ql.contains("прода") {
            (dist_n, "роздають")
        } else if ql.contains("нейтр") {
            (neu_n, "нейтральні")
        } else {
            (acc_n, "накопичують")
        };
        let pct = ((num as f64 / total as f64) * 100.0).round() as u64;
        hint = format!("\n\nПРЯМА ПІДКАЗКА-ФАКТ: візьми за ТОЧНУ ОСНОВУ й РОЗГОРНИ її конкретними числами з JSON:'{pct}% родин ({num} із {total}) зараз {what}.'");
    } else if contrarian_q {
        hint = "\n\nПРЯМА ПІДКАЗКА-ФАКТ: візьми за ТОЧНУ ОСНОВУ й РОЗГОРНИ її конкретними числами з JSON:'Ні, не автоматично. За нашими даними масове накопичення китів ПЕРЕДУЄ падінню ціни — сигнал часто інвертований і може бути пасткою. Не заходь наосліп слідом.'".into();
    } else if comp_word {
        let flow3 = |n: &str| ctx["ротаціяНаративів"][n]["останні3місяціНетто"].as_f64().unwrap_or(0.0);
        let want_out = ["втрача", "виход", "відтік", "скид", "слабк", "гірш"].iter().any(|k| ql.contains(k));
        let pick = if want_out {
            named_all.iter().copied().min_by(|a, b| flow3(a).partial_cmp(&flow3(b)).unwrap_or(std::cmp::Ordering::Equal))
        } else {
            named_all.iter().copied().max_by(|a, b| flow3(a).partial_cmp(&flow3(b)).unwrap_or(std::cmp::Ordering::Equal))
        };
        if let Some(p) = pick {
            let verb = if want_out { "більше втрачає (сильніший відтік капіталу)" } else { "сильніше зростає (більший притік капіталу)" };
            hint = format!("\n\nПРЯМА ПІДКАЗКА-ФАКТ: візьми за ТОЧНУ ОСНОВУ й РОЗГОРНИ її конкретними числами з JSON:'Наратив {p} зараз {verb}.'");
        }
    } else if who_sector {
        let sec = named.unwrap();
        let acc_verb = ["накопич", "набира", "тримают"].iter().any(|k| ql.contains(k));
        let field = if acc_verb { "архетипиЩоНабирають" } else { "архетипиЩоРоздають" };
        let labels: Vec<String> = ctx[field].as_array()
            .map(|a| a.iter().filter_map(|x| x["label"].as_str()).filter(|l| l.contains(sec)).map(|s| s.to_string()).collect())
            .unwrap_or_default();
        if labels.is_empty() {
            let vs = if acc_verb { "накопичує" } else { "роздає" };
            hint = format!("\n\nПРЯМА ПІДКАЗКА-ФАКТ: візьми за ТОЧНУ ОСНОВУ й РОЗГОРНИ її конкретними числами з JSON:'Жодна помітна родина зараз не {vs} {sec}.'");
        } else {
            hint = format!("\n\nПРЯМА ПІДКАЗКА-ФАКТ: відповідай РІВНО, назвавши архетип(и): '{}'.", labels.join("; "));
        }
    } else if top1_q {
        let dumping = ["розда", "скида", "прода"].iter().any(|k| ql.contains(k));
        let list = if dumping { &dist_syms_top } else { &acc_syms_top };
        let c = list.first().cloned().unwrap_or_else(|| "—".into());
        let what = if dumping { "скидання (роздачі)" } else { "накопичення" };
        hint = format!("\n\nПРЯМА ПІДКАЗКА-ФАКТ: візьми за ТОЧНУ ОСНОВУ й РОЗГОРНИ її конкретними числами з JSON:'Головна монета {what} в китів зараз — {c}.'");
    } else if domsec_q {
        hint = match ctx["домінантнийСекторРодин"].as_str() {
            Some(s) => format!("\n\nПРЯМА ПІДКАЗКА-ФАКТ: візьми за ТОЧНУ ОСНОВУ й РОЗГОРНИ її конкретними числами з JSON:'Серед китових родин домінує сектор {s}.'"),
            None => "\n\nПРЯМА ПІДКАЗКА-ФАКТ: візьми за ТОЧНУ ОСНОВУ й РОЗГОРНИ її конкретними числами з JSON:'Більшість родин не мають вираженого домінантного сектора.'".into(),
        };
    } else if compound_q {
        let coins = if acc_syms_top.is_empty() { "—".to_string() } else { acc_syms_top.join(", ") };
        let inf = ctx["наративКудиЗаходятьГрошіКитів"].as_str().unwrap_or("—");
        hint = format!("\n\nПРЯМА ПІДКАЗКА-ФАКТ: візьми за ТОЧНУ ОСНОВУ й РОЗГОРНИ її конкретними числами з JSON:'Кити купують {coins}, а капітал заходить у наратив {inf}.'");
    } else if coins_intent {
        let selling = ["скида", "розда", "прода"].iter().any(|k| ql.contains(k))
            && !["накопич", "набира", "купу", "заход"].iter().any(|k| ql.contains(k));
        let (syms, verb) = if selling { (&dist_syms_top, "скидають") } else { (&acc_syms_top, "накопичують") };
        let list = if syms.is_empty() { "—".to_string() } else { syms.join(", ") };
        hint = format!("\n\nПРЯМА ПІДКАЗКА-ФАКТ: візьми за ТОЧНУ ОСНОВУ й РОЗГОРНИ її конкретними числами з JSON:'Кити зараз {verb} монети: {list}.'");
    } else if membership_q {
        let t = q_ticker.unwrap();
        let syms = |f: &str| -> Vec<String> {
            ctx[f].as_array().map(|a| a.iter().filter_map(|x| x["токен"].as_str()).map(|s| s.to_uppercase()).collect()).unwrap_or_default()
        };
        if syms("монетиЩоНабирають").contains(&t) {
            hint = format!("\n\nПРЯМА ПІДКАЗКА-ФАКТ: візьми за ТОЧНУ ОСНОВУ й РОЗГОРНИ її конкретними числами з JSON:'Так, {t} серед токенів, які кити зараз накопичують.'");
        } else if syms("монетиЩоРоздають").contains(&t) {
            hint = format!("\n\nПРЯМА ПІДКАЗКА-ФАКТ: візьми за ТОЧНУ ОСНОВУ й РОЗГОРНИ її конкретними числами з JSON:'Навпаки — {t} кити зараз скидають, а не купують.'");
        } else {
            hint = format!("\n\nПРЯМА ПІДКАЗКА-ФАКТ: візьми за ТОЧНУ ОСНОВУ й РОЗГОРНИ її конкретними числами з JSON:'{t} нема серед токенів, активних у китів за нашими даними.'");
        }
    } else if let Some(n) = named {
        let key = if month_q { "останнійМісяцьНетто" } else { "останні3місяціНетто" };
        let net = ctx["ротаціяНаративів"][n][key].as_f64().unwrap_or(0.0);
        let per = if month_q { " за останній місяць" } else { "" };
        if net > 0.0 {
            hint = format!("\n\nПРЯМА ПІДКАЗКА-ФАКТ: візьми за ТОЧНУ ОСНОВУ й РОЗГОРНИ її конкретними числами з JSON:'Наратив {n}{per} — притік капіталу, кити заходять.'");
        } else if net < 0.0 {
            hint = format!("\n\nПРЯМА ПІДКАЗКА-ФАКТ: візьми за ТОЧНУ ОСНОВУ й РОЗГОРНИ її конкретними числами з JSON:'Наратив {n}{per} — відтік капіталу, кити виходять.'");
        } else {
            hint = format!("\n\nПРЯМА ПІДКАЗКА-ФАКТ: по наративу {n}{per} нема помітного чистого потоку капіталу.");
        }
    } else if outflow_intent && inflow_intent {
        let in_s = inflow.map(|n| format!("заходять у наратив {n}")).unwrap_or_else(|| "нікуди (чистого притоку нема)".into());
        let out_s = outflow.map(|n| format!("виходять з наративу {n}")).unwrap_or_else(|| "нізвідки (чистого відтоку нема)".into());
        hint = format!("\n\nПРЯМА ПІДКАЗКА-ФАКТ: візьми за ТОЧНУ ОСНОВУ й РОЗГОРНИ її конкретними числами з JSON:'Гроші китів зараз {in_s}, а {out_s}.'");
    } else if outflow_intent {
        hint = match outflow {
            Some(n) => format!("\n\nПРЯМА ПІДКАЗКА-ФАКТ: візьми за ТОЧНУ ОСНОВУ й РОЗГОРНИ її конкретними числами з JSON:'Гроші китів зараз виходять з наративу {n}.' Додай повну картину потоку з ротаціяНаративів (нетто-числа по кожному наративу)."),
            None => "\n\nПРЯМА ПІДКАЗКА-ФАКТ: візьми за ТОЧНУ ОСНОВУ й РОЗГОРНИ її конкретними числами з JSON:'Чистого відтоку в жодному наративі зараз нема.'".into(),
        };
    } else if inflow_intent {
        hint = match inflow {
            Some(n) => format!("\n\nПРЯМА ПІДКАЗКА-ФАКТ: візьми за ТОЧНУ ОСНОВУ й РОЗГОРНИ її конкретними числами з JSON:'Гроші китів зараз заходять у наратив {n}.' Додай повну картину потоку з ротаціяНаративів (нетто-числа по кожному наративу)."),
            None => "\n\nПРЯМА ПІДКАЗКА-ФАКТ: візьми за ТОЧНУ ОСНОВУ й РОЗГОРНИ її конкретними числами з JSON:'Чистого притоку в жоден наратив зараз нема.'".into(),
        };
    } else if advice_q {
        // advice/summary/trend — assemble a CONCISE grounded answer to stop 7b from
        // rambling and coining nonsense words on open-ended prompts.
        let top3 = acc_syms_top.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
        let inf = inflow.unwrap_or("—");
        let outf = outflow.unwrap_or("—");
        let bigger_short = if acc_n > dist_n { "НАБИРАЮТЬ (накопичують)" } else if dist_n > acc_n { "РОЗДАЮТЬ (продають)" } else { "нейтральні" };
        hint = format!("\n\nПРЯМА ПІДКАЗКА-ФАКТ: дай РОЗГОРНУТУ пораду (4-6 речень) на основі цих фактів, з КОНКРЕТНИМИ числами з JSON: кити зараз більше {bigger_short} ({acc_n} набирають / {dist_n} роздають із {enriched_n} збагачених родин); капітал заходить у наратив {inf}, виходить з {outf} (нетто-числа візьми з поля ротаціяНаративів); топ-монети накопичення — {top3} (частоти родин — з монетиЩоНабирають). Обов'язково заверши застереженням: масове накопичення часто ПЕРЕДУЄ падінню ціни, тож НЕ автоматичний бичачий сигнал. БЕЗ вигаданих слів і чисел поза JSON.");
    } else {
        // fallback: без цього хінт лишався порожнім і 7b розсипався дампом усіх полів+9 принципів.
        let bs = if acc_n > dist_n { "більше НАБИРАЮТЬ (накопичують)" } else if dist_n > acc_n { "більше РОЗДАЮТЬ (продають)" } else { "приблизно нейтральні" };
        let inf = inflow.unwrap_or("—");
        let outf = outflow.unwrap_or("—");
        unmatched = true;   // питання поза відомими інтентами — якір НЕ навʼязуємо
    }

    // Найчастіші питання про дані мають ОДНОЗНАЧНУ відповідь із контексту —
    // модель для них не потрібна й лише псує числа. Віддаємо детерміновано.
    if let Some(t) = crate::direct::try_answer(&question, &ctx) {
        return Json(json!({"configured": true, "source": "direct", "answer": t}))
            .into_response();
    }

    // Питання не лягло в жоден інтент → даємо моделі ІНСТРУМЕНТИ, хай дістане сама.
    // Раніше тут спрацьовував один якір про наративи, і всі такі питання отримували
    // ту саму відповідь незалежно від змісту.
    if unmatched {
        if let Some(t) = crate::tools::run(&question, &ctx, &base, &model, &key).await {
            return Json(json!({
                "configured": true,
                "source": "tools",
                "answer": t
            })).into_response();
        }
    }

    let system = "Ти — AI-аналітик Wallet Radar (відстеження китових гаманців). Відповідай РОЗГОРНУТО, ТОЧНО і лише українською, спираючись на КОНКРЕТНІ дані з наданого JSON. \
СТРУКТУРА ЗМІСТОВНОЇ ВІДПОВІДІ (3-6 речень, НЕ більше): 1) прямий висновок; 2) конкретні ДАНІ-ПІДСТАВИ з JSON саме до цього питання — числа, лічильники родин, нетто-потоки, частоти монет; 3) коротка інтерпретація. 'РОЗГОРНУТО' означає 3-6 речень ПО ТЕМІ питання — це НЕ звіт на всі поля JSON. Відповідай ЛИШЕ на поставлене питання. НЕ дампи топРодини/архетипи/усі наративи, якщо про них прямо не спитали. НЕ перелічуй принципи лінзи (максимум 1-2 і ЛИШЕ на питання про стратегію/ризик/довіру). ВИНЯТОК: відмови ('таких даних нема', 'не прогнозую ціни') лишай КОРОТКИМИ. \
ПРАВИЛА: \
1) Спирайся ВИКЛЮЧНО на наданий JSON. Не вигадуй чисел, назв, адрес, слів, пояснень. Якщо даних нема — чесно 'таких даних у мене нема'. ПРАВДИВІСТЬ ПОНАД УСЕ: краще визнати брак даних, ніж вгадати. Усі числа бери ЛИШЕ з JSON. \
2) Коли в підказці є 'ПРЯМА ПІДКАЗКА-ФАКТ' — це ТОЧНИЙ якір відповіді. Почни з нього, а тоді РОЗГОРНИ його конкретними числами з JSON. Не супереч якорю. \
3) Напрям фаз ('купують чи продають') — з 'фазиРодин.висновок'; підкріпи лічильниками набирають/роздають/нейтральні. Монети — СИМВОЛИ з 'монетиЩоНабирають'/'монетиЩоРоздають' (з частотами кількістьРодин), а НЕ мітки архетипів. Стейбли вже прибрані. \
4) НЕ прогнозуй ціни/курси/майбутнє. На 'спрогнозуй / ціна завтра / скільки коштуватиме / що буде з ціною' відповідай КОРОТКО й дослівно: 'Я не прогнозую ціни — я показую лише потоки китів у наявних даних.' (БЕЗ розгортання). \
5) Потік капіталу по наративах ('куди/звідки течуть гроші', 'гарячий/втрачає наратив') — з 'наративКудиЗаходятьГрошіКитів'/'наративЗвідкиВиходятьГрошіКитів' + нетто-числа з 'ротаціяНаративів'. Це ІНШЕ, ніж фази родин; 'other' ігноруй. Для розгортання наведи нетто по кількох наративах. \
5а) ЗНАК НЕТТО (критично, не помились): у 'ротаціяНаративів' ДОДАТНЄ число = ПРИТІК (кити ЗАХОДЯТЬ), ВІД'ЄМНЕ число (зі знаком '−') = ВІДТІК (кити ВИХОДЯТЬ). КОЖНЕ число має ВЛАСНУ мітку напряму того ж періоду: 'останнійМісяцьНетто' → бери 'напрямМісяць'; 'останні3місяціНетто' → бери 'напрям3місяці'. НЕ переноси мітку одного періоду на число іншого: місячне й квартальне нетто можуть мати РІЗНІ знаки (напр. місяць −1085 = відтік, а 3 місяці +749 = притік — це нормально й НЕ суперечність). НІКОЛИ не називай від'ємне число притоком. Якщо цитуєш нетто — став поруч мітку саме його періоду. \
6) Фази українською: 'набирає'=накопичує(купує/тримає), 'роздає'=продає. НЕ плутай і не супереч собі. Архетип — мітка типу родини; топЕмісія — токени й частка емісії; сектор — домінантний наратив. Для 'найбільша родина' бери 'найбільшаРодина' як є. \
7) 'принципиТоргівлі(лінзаЛьюіс)' — інтерпретаційна лінза (уроки книг: Льюіс 'Покер брехунів', Лівермор, Ґрем, Маккей), а НЕ факти про дані. Наводь 1-2 доречні уроки ЛИШЕ на питання про стратегію/ризик/довіру/'як діяти', разом із конкретним числом із JSON. Не перелічуй усі; не видавай принцип за поточну подію.";
    let user = format!(
        "Питання: {question}\n\nДані застосунку (JSON, українські ключі):\n{}{}\n\n[МОВА] Відповідь дай ВИКЛЮЧНО українською мовою, без жодного російського слова.",
        serde_json::to_string(&ctx).unwrap_or_default(),
        hint
    );

    let body = json!({
        "model": model,
        "max_tokens": 600,
        "temperature": 0.0,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user}
        ]
    });

    let client = reqwest::Client::new();
    let url = format!("{}/chat/completions", base.trim_end_matches('/'));
    let resp = client
        .post(&url)
        .header("authorization", format!("Bearer {key}"))
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await;

    match resp {
        Ok(r) => {
            let status = r.status();
            let v: Value = match r.json().await {
                Ok(v) => v,
                Err(e) => return Json(json!({"error": format!("некоректна відповідь LLM: {e}")})).into_response(),
            };
            if !status.is_success() || v.get("error").is_some() {
                let msg = v["error"]["message"].as_str().or_else(|| v["error"].as_str()).unwrap_or("LLM error");
                return Json(json!({"error": format!("LLM {}: {}", status.as_u16(), msg)})).into_response();
            }
            let answer = v["choices"][0]["message"]["content"].as_str().unwrap_or("").trim().to_string();
            if answer.is_empty() {
                return Json(json!({"configured": true, "answer": "(порожня відповідь)", "model": v["model"]})).into_response();
            }
            // Багатомовність: legacy-конвеєр рахує факт українською; для НЕ-укр запиту
            // перекладаємо фінальну відповідь мовою запиту (число-гард: якщо переклад
            // спотворює якесь велике число — лишаємо український оригінал).
            let lang = kb_detect_lang(&question);
            let out = if lang != "uk" {
                kb_translate(&base, &model, &key, &answer, lang).await.unwrap_or(answer)
            } else { answer };
            Json(json!({"configured": true, "answer": out, "model": v["model"], "lang": lang})).into_response()
        }
        Err(e) => Json(json!({"error": format!("запит до LLM не вдався: {e}")})).into_response(),
    }
}
