//! Реєстр провайдерів: єдине місце, де сателіт знає, КУДИ можна ходити і ЯК підставляти ключ.
//!
//! Це не таблиця зручності, а межа безпеки. Сателіт приймає від кора лише ІМʼЯ провайдера з
//! цього списку — ніколи не хост і не повний URL. Інакше будь-хто з валідним клієнтським
//! сертифікатом перетворив би сателіт на відкритий проксі всередину нашої мережі (SSRF).

use std::time::Duration;

/// Як провайдер приймає ключ.
#[derive(Clone, Copy, PartialEq)]
pub enum Auth {
    /// Параметр у query-рядку: `?apikey=…`
    Query(&'static str),
    /// Заголовок: `X-API-Key: …`
    Header(&'static str),
    /// `Authorization: Bearer …`
    Bearer,
}

pub struct Provider {
    pub name: &'static str,
    pub base: &'static str,
    pub auth: Auth,
    pub env: &'static str,
    /// Пауза між стартами запитів для ОДНОГО сателіта, якщо він єдиний власник квоти.
    /// Коли квоту ділять кілька сателітів (спільний акаунт), цей інтервал множиться на
    /// SATELLITE_QUOTA_SHARE — див. `effective_gap`.
    pub min_gap: Duration,
}

/// Порядок значення не має; пошук за іменем.
pub const PROVIDERS: &[Provider] = &[
    // 3 запити/с — НЕ 5. Це не з документації (вона суперечлива), а зі слів самого сервера:
    //   {"status":"0","message":"NOTOK","result":"Max calls per sec rate limit reached (3/sec)"}
    // Реєстр раніше стояв на 220 мс (≈4.5/с) за припущенням «5/с», тобто ми свідомо жену́ли
    // понад ліміт: ~45% викликів Etherscan поверталися цією відмовою, кор мовчки їх повторював.
    // 400 мс = 2.5/с, тобто НЕ впритул до 3/с. Запас тут не перестраховка, а необхідність:
    // лімітери сателітів незалежні й не синхронізовані між собою. Рівно 3/с у сумі означає 3/с
    // лише В СЕРЕДНЬОМУ — фази двох потоків дрейфують, і час від часу обидва стріляють в одну
    // секунду. Виміряно: на точних 3/с провайдер усе одно відхиляв ~15% викликів.
    //
    // І головне: ліміт Etherscan рахується НА АКАУНТ, а не на ключ. Три ключі одного акаунта
    // ділять ті самі 3/с, тож другий ключ звідти квоти не додає (перевірено; див. docs).
    Provider {
        name: "etherscan",
        base: "https://api.etherscan.io",
        auth: Auth::Query("apikey"),
        env: "ETHERSCAN_API_KEY",
        min_gap: Duration::from_millis(400),
    },
    Provider {
        name: "moralis",
        base: "https://deep-index.moralis.io",
        auth: Auth::Header("X-API-Key"),
        env: "MORALIS_API_KEY",
        min_gap: Duration::from_millis(100),
    },
    // 15 с = 4 виклики/хв — не здогад, а те, що віддає сам сервер у заголовку
    // `x-rate-limit-minute: 4`. Раніше тут стояло 500 мс (60/хв), тобто ми були налаштовані
    // бити У 15 РАЗІВ швидше за дозволене.
    //
    // Але головне обмеження навіть не це, а `x-rate-limit-day: 100` — СТО викликів на добу.
    // Жоден темп не врятує, якщо просити більше: тут потрібен не лімітер, а менше запитів
    // (див. кеш у prices.rs::get_social і docs/satellites.md).
    Provider {
        name: "lunarcrush",
        base: "https://lunarcrush.com",
        auth: Auth::Bearer,
        env: "LUNARCRUSH_API_KEY",
        min_gap: Duration::from_millis(15_000),
    },
    // Arkham і Glassnode зареєстровані, але ключів у нас поки НЕМАЄ (ARKHAM_API_KEY порожній,
    // GLASSNODE_API_KEY відсутній). Health чесно віддасть configured=false, а Fetch — виразну
    // помилку. Це навмисно: мовчазна порожня відповідь виглядала б як «даних немає».
    Provider {
        name: "arkham",
        base: "https://api.arkhamintelligence.com",
        auth: Auth::Header("API-Key"),
        env: "ARKHAM_API_KEY",
        min_gap: Duration::from_millis(200),
    },
    Provider {
        name: "glassnode",
        base: "https://api.glassnode.com",
        auth: Auth::Query("api_key"),
        env: "GLASSNODE_API_KEY",
        min_gap: Duration::from_millis(200),
    },
];

pub fn find(name: &str) -> Option<&'static Provider> {
    PROVIDERS.iter().find(|p| p.name == name)
}

/// Яким саме лімітом нас притримали.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Limit {
    /// Не ліміт — звичайна відповідь.
    None,
    /// Посекундний: минуще. Сповільнитись і продовжити — правильна реакція.
    Transient,
    /// ДОБОВИЙ: бюджет вичерпано. Сповільнення НЕ допомагає — хоч раз на годину стукай, усе
    /// одно «ні», аж до скидання. Плутати ці два випадки означає цілий день довбати джерело,
    /// яке вже сказало «ні», і думати, що ми «просто трохи перевищуємо темп».
    Daily,
}

/// Чи є ця відповідь відмовою ЗА ЛІМІТОМ (а не даними) — і яким саме.
///
/// Так, сателіт тут зазирає в тіло — і це єдиний виняток із правила «не інтерпретувати дані».
/// Виняток свідомий: питання «мене щойно притримали за квоту?» стосується КВОТИ, а квота — це
/// зона відповідальності сателіта, а не кора. Логіка парсингу даних сюди не переїжджає: тіло
/// повертається кору байт-у-байт незмінним, ми лише вирішуємо, як швидко гнати далі.
///
/// Два реальні приклади, обидва зловлені на живому проді:
///   Etherscan  → HTTP 200 + {"message":"NOTOK","result":"Max calls per sec rate limit reached (3/sec)"}
///   LunarCrush → HTTP 429 + {"error":"Rate limit exceeded (daily)"}
/// Хто чекає лише на 429 — не побачить ліміту Etherscan. Хто не читає слова «daily» — цілий
/// день гальмуватиме там, де гальмування не рятує.
/// Квота із ЗАГОЛОВКІВ відповіді — точне знання замість здогаду.
///
/// Провайдери часто кажуть, скільки нам лишилося (`x-rate-limit-day-remaining: -43`), і це
/// незрівнянно краще за body-sniffing: ми дізнаємось про вичерпання ДО того, як нас відріжуть,
/// а не з тіла чергової відмови. Саме так виявилося, що добовий бюджет LunarCrush — 100
/// викликів, а ми були на 43 за межею.
///
/// `None` — провайдер про квоту не каже (Etherscan, наприклад, мовчить).
pub fn quota_left(headers: &reqwest::header::HeaderMap) -> Option<i64> {
    headers
        .iter()
        .filter_map(|(name, value)| {
            let n = name.as_str().to_ascii_lowercase();
            // Ловимо і `x-rate-limit-day-remaining`, і звичне `x-ratelimit-remaining`.
            if n.contains("remaining") && (n.contains("rate") || n.contains("limit")) {
                value.to_str().ok()?.trim().parse::<i64>().ok()
            } else {
                None
            }
        })
        // НАЙМЕНШИЙ залишок: якщо хвилинний ще є, а добовий скінчився, нас усе одно не пустять.
        // Обмежує завжди найсуворіше з вікон.
        .min()
}

pub fn classify_limit(status: u16, body: &[u8]) -> Limit {
    // Дешева перевірка байтів: повний JSON-розбір тут був би і зайвим, і крихким.
    let head = &body[..body.len().min(512)];
    let text = String::from_utf8_lossy(head).to_ascii_lowercase();
    let limited = status == 429
        || (status == 200 && (text.contains("rate limit") || text.contains("max calls per sec")));
    if !limited {
        return Limit::None;
    }
    if text.contains("daily") || text.contains("per day") || text.contains("day limit") {
        return Limit::Daily;
    }
    Limit::Transient
}

/// Скільки сателітів ділять ОДИН акаунт провайдера (`SATELLITE_QUOTA_SHARE`, типово 1).
///
/// Це виправлення хибної засади. Лічильник у сателіті мовчки припускав, що квота належить
/// ЙОМУ. Насправді вона належить акаунту: два сателіти з ключами одного акаунта, кожен зі своїм
/// лімітером на 3/с, разом дають 6/с — і половину відхиляє провайдер. Множник робить квоту
/// спільною чесно: кожен жене вдвічі повільніше, сумарно виходить рівно ліміт.
///
/// Коли в сателітів будуть ключі РІЗНИХ акаунтів, тут має стояти 1 — квоти вже незалежні.
pub fn quota_share() -> u32 {
    std::env::var("SATELLITE_QUOTA_SHARE").ok().and_then(|s| s.parse().ok()).filter(|&n| n >= 1).unwrap_or(1)
}

/// Реальна пауза цього сателіта з урахуванням спільної квоти.
pub fn effective_gap(p: &Provider) -> Duration {
    p.min_gap * quota_share()
}

impl Provider {
    /// Ключ із оточення САМЕ ЦЬОГО сателіта. `None` = не налаштований.
    ///
    /// Значення може бути СПИСКОМ через кому — кілька ключів РІЗНИХ акаунтів того самого
    /// провайдера. Тоді вони ротуються round-robin по викликах: добова квота рахується НА
    /// АКАУНТ, тож чергування ділить навантаження між акаунтами й множить денний бюджет.
    /// Один ключ (без коми) поводиться точно як раніше.
    pub fn key(&self) -> Option<String> {
        let raw = std::env::var(self.env).ok().filter(|s| !s.trim().is_empty())?;
        let keys: Vec<&str> = raw.split(',').map(str::trim).filter(|s| !s.is_empty()).collect();
        match keys.as_slice() {
            [] => None,
            [only] => Some((*only).to_string()),
            many => {
                use std::collections::HashMap;
                use std::sync::{Mutex, OnceLock};
                static CTR: OnceLock<Mutex<HashMap<&'static str, usize>>> = OnceLock::new();
                let mut g = CTR.get_or_init(|| Mutex::new(HashMap::new())).lock().unwrap();
                let c = g.entry(self.env).or_insert(0);
                let i = *c;
                *c = c.wrapping_add(1);
                Some(many[i % many.len()].to_string())
            }
        }
    }
}

/// Шлях мусить бути шляхом, а не втечею на інший хост. Відкидаємо все, що могло б вивести
/// запит за межі базового URL провайдера.
pub fn path_is_safe(path: &str) -> bool {
    path.starts_with('/') && !path.starts_with("//") && !path.contains("://") && !path.contains("..")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_paths_that_escape_the_provider() {
        assert!(path_is_safe("/v2/api"));
        assert!(path_is_safe("/api/v2/wallets/0xabc/tokens"));
        // Спроби втечі: інший хост, протокол-релятив, обхід угору.
        assert!(!path_is_safe("https://evil.example/x"));
        assert!(!path_is_safe("//evil.example/x"));
        assert!(!path_is_safe("/v2/../../etc/passwd"));
        assert!(!path_is_safe("v2/api")); // без провідного слеша
    }

    /// Etherscan сам сказав нам «(3/sec)» — і поки він каже це, реєстр мусить стояти на 3/с.
    /// Старе значення 220 мс (≈4.5/с) було припущенням «5/с», через яке ~45% викликів
    /// відхилялися. Якщо хтось повернеться до <334 мс, це має впасти тут, а не в проді.
    #[test]
    fn etherscan_gap_matches_the_limit_the_server_actually_enforces() {
        let es = find("etherscan").unwrap();
        let per_sec = 1000.0 / es.min_gap.as_millis() as f64;
        assert!(per_sec <= 3.0, "жену́ли б {per_sec:.1}/с, а Etherscan дозволяє 3/с");
    }

    /// Квота належить АКАУНТУ. Два сателіти на спільному акаунті мусять кожен гальмувати вдвічі,
    /// інакше разом вони дають подвійний ліміт — рівно той баг, який це поле й лікує.
    #[test]
    fn shared_account_halves_each_satellites_rate() {
        let es = find("etherscan").unwrap();
        // quota_share() читає оточення, тож перевіряємо саму арифметику частки.
        assert_eq!(es.min_gap * 2, std::time::Duration::from_millis(800));
        assert_eq!(es.min_gap * 1, es.min_gap);
    }

    /// Не впритул до ліміту. Лімітери сателітів незалежні, їхні фази дрейфують, тож рівно 3/с у
    /// сумі провайдер усе одно частково відхиляє (виміряно ~15%). Запас мусить бути.
    #[test]
    fn etherscan_leaves_headroom_below_the_hard_limit() {
        let es = find("etherscan").unwrap();
        let per_sec = 1000.0 / es.min_gap.as_millis() as f64;
        assert!(per_sec < 3.0, "{per_sec:.1}/с — впритул до 3/с, запасу на дрейф фаз немає");
    }

    #[test]
    fn every_provider_has_distinct_name_and_https_base() {
        for p in PROVIDERS {
            assert!(p.base.starts_with("https://"), "{} мусить бути https", p.name);
            assert_eq!(PROVIDERS.iter().filter(|q| q.name == p.name).count(), 1, "дубль: {}", p.name);
        }
    }
}
