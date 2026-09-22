//! Сателіт: gRPC-сервер поверх mTLS, який тримає ключі до зовнішніх API і ходить у них
//! замість кора.
//!
//! Навіщо: (1) ключів на корі не лишається взагалі — зламаний кор не віддає нічого цінного;
//! (2) кожен сателіт має власний IP, а Blockscout та інші лімітують саме за IP;
//! (3) квоту МОЖНА помножити — але лише ключами з РІЗНИХ акаунтів.
//!
//! Пункт (3) спершу був сформульований хибно («квота прив'язана до КЛЮЧА, тож N ключів =
//! N×квота»), і на цій хибі трималася вся конструкція. Etherscan рахує ліміт НА АКАУНТ (і не
//! 5/с, а 3/с — так каже сам сервер у тілі відмови), а один акаунт дає щонайбільше 3 ключі, які
//! ділять ті самі 3/с. Тому два сателіти з ключами одного акаунта не подвоюють нічого — вони
//! лише разом б'ють понад ліміт, якщо кожен вважає квоту своєю. Звідси SATELLITE_QUOTA_SHARE:
//! див. registry::effective_gap.
//!
//! Сателіт НЕ відкритий проксі: він приймає лише імена провайдерів зі свого реєстру
//! (див. registry.rs), ніколи не хост і не повний URL.

mod registry;

use registry::{path_is_safe, Auth};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tonic::transport::{Certificate, Identity, Server, ServerTlsConfig};
use tonic::{Request, Response, Status};

pub mod pb {
    tonic::include_proto!("satellite.v1");
}
use pb::upstream_server::{Upstream, UpstreamServer};
use pb::{FetchRequest, FetchResponse, HealthRequest, HealthResponse, ProviderStatus};

/// Стеля на тіло, яке ми віддаємо кору; має збігатися з `MAX_MSG` на його боці. Дефолтні 4 МіБ
/// tonic різали довгі txlist-відповіді ще на кодуванні, і кор отримував не помилку, а обрив.
const MAX_MSG: usize = 64 * 1024 * 1024;

/// Серіалізує виклики ОДНОГО провайдера й САМ ЗНАХОДИТЬ його реальну стелю (AIMD).
///
/// Чому не константа. Ліміт Etherscan ми двічі брали з документації й двічі мали хибне число
/// (5/с замість 3/с), а потім поставили «чесні» 2.5/с — і все одно ~7% викликів відхилялося.
/// Стеля залежить від тарифу, ендпойнта, сусідів по IP і настрою провайдера, і змінюється без
/// попередження. Будь-яка вбита в код константа рано чи пізно стає хибною МОВЧКИ — а мовчазна
/// хиба тут коштує 45% викликів, як уже й було.
///
/// Тому: відмова за лімітом → пауза × 1.5 (швидко відступаємо), серія успіхів → пауза
/// поступово повертається до базової (обережно наступаємо). Лімітер сам сходиться до того
/// темпу, який провайдер СПРАВДІ приймає, і переживає зміну ліміту без нашого втручання.
struct Limiter {
    gates: Mutex<HashMap<&'static str, Gate>>,
}

struct Gate {
    /// Коли можна стартувати наступний запит.
    next: Instant,
    /// Поточна пауза — не менша за базову, не більша за base × MAX_BACKOFF.
    gap: Duration,
    /// Успіхів поспіль від останньої відмови.
    wins: u32,
}

/// Наскільки максимум дозволяємо собі загальмувати. Без стелі один поганий період провайдера
/// загнав би сателіт у багатохвилинні паузи, з яких він уже не вибереться.
const MAX_BACKOFF: u32 = 8;
/// Скільки успіхів поспіль потрібно, щоб наважитися пришвидшитися на крок.
const WINS_TO_SPEED_UP: u32 = 15;

impl Limiter {
    fn new() -> Self {
        Self { gates: Mutex::new(HashMap::new()) }
    }

    /// Резервує наступний слот і повертає, скільки треба почекати перед стартом.
    fn reserve(&self, provider: &'static str, base: Duration) -> Duration {
        let mut g = self.gates.lock().unwrap();
        let now = Instant::now();
        let gate = g.entry(provider).or_insert(Gate { next: now, gap: base, wins: 0 });
        let start = now.max(gate.next);
        gate.next = start + gate.gap;
        start.saturating_duration_since(now)
    }

    /// Результат виклику: провайдер притримав нас — чи прийняв.
    fn observe(&self, provider: &'static str, base: Duration, limit: registry::Limit) {
        let mut g = self.gates.lock().unwrap();
        let Some(gate) = g.get_mut(provider) else { return };
        let ceiling = base * MAX_BACKOFF;

        // Добова квота вичерпана. Сповільнення тут НЕ рятує — до скидання відповідь буде та сама,
        // хоч раз на секунду стукай, хоч раз на хвилину. Тому одразу відступаємо на стелю (щоб не
        // довбати джерело весь день) і кажемо про це ГОЛОСНО: інакше це виглядає як «трохи
        // перевищуємо темп», хоча насправді провайдер для нас закритий до завтра.
        if limit == registry::Limit::Daily {
            gate.wins = 0;
            if gate.gap < ceiling {
                tracing::error!(
                    "[{provider}] ДОБОВА квота вичерпана — сповільнення не допоможе. Відступаю на {} мс до скидання. Дані з цього джерела зараз НЕ надходять",
                    ceiling.as_millis()
                );
                gate.gap = ceiling;
            }
            return;
        }

        if limit == registry::Limit::Transient {
            gate.wins = 0;
            let widened = gate.gap.mul_f32(1.5).min(ceiling);
            if widened > gate.gap {
                tracing::warn!(
                    "[{provider}] відмова за лімітом → сповільнююсь {} → {} мс",
                    gate.gap.as_millis(),
                    widened.as_millis()
                );
                gate.gap = widened;
            }
            return;
        }
        gate.wins += 1;
        if gate.wins >= WINS_TO_SPEED_UP && gate.gap > base {
            gate.wins = 0;
            // Наступаємо ДОДАВАНКОЮ, а не множенням: відступати треба різко, повертатися —
            // повільно, інакше лімітер коливатиметься навколо стелі замість зійтися до неї.
            let narrowed = gate.gap.saturating_sub(base / 4).max(base);
            tracing::info!(
                "[{provider}] {WINS_TO_SPEED_UP} успіхів поспіль → пришвидшуюсь {} → {} мс",
                gate.gap.as_millis(),
                narrowed.as_millis()
            );
            gate.gap = narrowed;
        }
    }

    /// Пауза, на якій лімітер стоїть ЗАРАЗ (для Health — інакше він показував би базову).
    fn current_gap(&self, provider: &'static str, base: Duration) -> Duration {
        self.gates.lock().unwrap().get(provider).map_or(base, |g| g.gap)
    }
}

struct Sat {
    id: String,
    http: reqwest::Client,
    limiter: Limiter,
    calls: Mutex<HashMap<&'static str, Arc<AtomicU64>>>,
}

impl Sat {
    fn counter(&self, name: &'static str) -> Arc<AtomicU64> {
        self.calls.lock().unwrap().entry(name).or_insert_with(|| Arc::new(AtomicU64::new(0))).clone()
    }
}

#[tonic::async_trait]
impl Upstream for Arc<Sat> {
    async fn fetch(&self, req: Request<FetchRequest>) -> Result<Response<FetchResponse>, Status> {
        let r = req.into_inner();

        let p = registry::find(&r.provider)
            .ok_or_else(|| Status::invalid_argument(format!("невідомий провайдер: {}", r.provider)))?;
        if !path_is_safe(&r.path) {
            // Не «підчистити й піти далі» — саме відмовити: шлях, що намагається втекти за межі
            // провайдера, це не помилка форматування, це спроба зробити з нас проксі.
            return Err(Status::invalid_argument(format!("небезпечний шлях: {}", r.path)));
        }
        let key = p.key().ok_or_else(|| {
            // Явна відмова, а не порожня відповідь: кор мусить БАЧИТИ, що ключа немає,
            // інакше він прийме тишу за «даних немає».
            Status::failed_precondition(format!("провайдер {} не налаштований на цьому сателіті (немає {})", p.name, p.env))
        })?;

        let mut url = url::Url::parse(p.base)
            .map_err(|e| Status::internal(format!("хибний base у реєстрі: {e}")))?
            .join(&r.path)
            .map_err(|e| Status::invalid_argument(format!("хибний шлях: {e}")))?;
        // Подвійний захист: join() із абсолютним URL міг би підмінити хост, тож звіряємо явно.
        if url.host_str() != url::Url::parse(p.base).ok().and_then(|b| b.host_str().map(str::to_string)).as_deref() {
            return Err(Status::invalid_argument("шлях змінює хост провайдера"));
        }
        {
            let mut q = url.query_pairs_mut();
            for (k, v) in &r.query {
                q.append_pair(k, v);
            }
            if let Auth::Query(param) = p.auth {
                q.append_pair(param, &key);
            }
        }

        // Ліміт провайдера — до відправки. Саме effective_gap, а не p.min_gap: коли акаунт
        // спільний, наша частка квоти менша, ніж увесь ліміт провайдера. Це лише БАЗА —
        // фактичну паузу лімітер добирає сам за відповідями провайдера.
        let base = registry::effective_gap(p);
        let wait = self.limiter.reserve(p.name, base);
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }

        let mut rq = self.http.get(url.clone()).header("accept", "application/json");
        rq = match p.auth {
            Auth::Header(h) => rq.header(h, &key),
            Auth::Bearer => rq.bearer_auth(&key),
            Auth::Query(_) => rq,
        };

        let resp = rq.send().await.map_err(|e| Status::unavailable(format!("{} недоступний: {e}", p.name)))?;
        let status = resp.status().as_u16() as u32;
        // Квота із заголовків — ДО того, як тіло з'їсть resp.
        let quota = registry::quota_left(resp.headers());
        let body = resp.bytes().await.map_err(|e| Status::unavailable(format!("тіло від {}: {e}", p.name)))?;

        self.counter(p.name).fetch_add(1, Ordering::Relaxed);

        // Єдине, що сателіт «читає» у тілі — чи не притримали його за квотою. Це знання про
        // КВОТУ, не про дані, і воно лишається тут, де живе ключ.
        let mut limit = registry::classify_limit(status as u16, &body);

        // Заголовок точніший за тіло: провайдер каже залишок ЩЕ ДО того, як почне відмовляти.
        // Нуль або менше — бюджет вичерпано, і наступні виклики марні, хоч би що було в тілі.
        if let Some(left) = quota {
            if left <= 0 {
                limit = registry::Limit::Daily;
            } else if left <= 10 {
                // Попереджаємо ЗАЗДАЛЕГІДЬ. Дізнатися про вичерпаний бюджет із відмови — це
                // дізнатися запізно: дані вже не надійшли.
                tracing::warn!("[{}] {} : квота добігає кінця — лишилось {left} викликів", self.id, p.name);
            }
        }

        self.limiter.observe(p.name, base, limit);

        tracing::info!(
            "[{}] {} {} → {} ({} байт){}",
            self.id,
            p.name,
            r.path,
            status,
            body.len(),
            match limit {
                registry::Limit::Daily => " ЛІМІТ-ДОБОВИЙ",
                registry::Limit::Transient => " ЛІМІТ",
                registry::Limit::None => "",
            }
        );

        // Статус і тіло віддаємо ЯК Є — навіть відмову за лімітом. Сателіт не інтерпретує дані
        // й не підміняє їх: інакше він став би другим місцем, де живе логіка парсингу, і два
        // місця неминуче розійшлися б. Кор сам вирішує, що робити з NOTOK.
        Ok(Response::new(FetchResponse { status, body: body.to_vec() }))
    }

    async fn health(&self, _: Request<HealthRequest>) -> Result<Response<HealthResponse>, Status> {
        let providers = registry::PROVIDERS
            .iter()
            .map(|p| ProviderStatus {
                name: p.name.to_string(),
                configured: p.key().is_some(),
                // Пауза, на якій лімітер стоїть ЗАРАЗ — не базова й не «ефективна за формулою».
                // Health, що показує намір замість факту, гірший за відсутній: саме за цим
                // числом видно, що провайдер притискає нас сильніше, ніж ми думали.
                min_gap_ms: self.limiter.current_gap(p.name, registry::effective_gap(p)).as_millis() as u32,
                calls_total: self.counter(p.name).load(Ordering::Relaxed),
            })
            .collect();
        Ok(Response::new(HealthResponse { satellite_id: self.id.clone(), providers }))
    }
}

#[cfg(test)]
mod limiter_tests {
    use super::*;

    const BASE: Duration = Duration::from_millis(400);

    #[test]
    fn backs_off_when_the_provider_pushes_back() {
        let l = Limiter::new();
        l.reserve("etherscan", BASE);
        assert_eq!(l.current_gap("etherscan", BASE), BASE);

        // Одна відмова — і ми вже гальмуємо. Чекати «серії» не можна: кожен зайвий запит
        // понад ліміт провайдер однаково відхилить, тобто це чистий збиток.
        l.observe("etherscan", BASE, registry::Limit::Transient);
        assert!(l.current_gap("etherscan", BASE) > BASE, "після відмови мусимо сповільнитись");

        // Стеля існує: інакше поганий період провайдера загнав би нас у паузи, з яких не вийти.
        for _ in 0..50 {
            l.observe("etherscan", BASE, registry::Limit::Transient);
        }
        assert_eq!(l.current_gap("etherscan", BASE), BASE * MAX_BACKOFF, "гальмування мусить мати стелю");
    }

    /// Добова квота — не «трохи швидко їдемо». Повзти на 1.5× повільніше безглуздо: до скидання
    /// відповідь та сама. Відступаємо на стелю ОДРАЗУ, а не за десяток марних кроків.
    #[test]
    fn daily_exhaustion_jumps_straight_to_the_ceiling() {
        let l = Limiter::new();
        l.reserve("lunarcrush", BASE);
        l.observe("lunarcrush", BASE, registry::Limit::Daily);
        assert_eq!(
            l.current_gap("lunarcrush", BASE),
            BASE * MAX_BACKOFF,
            "добова відмова мусить одразу відкинути на стелю, а не гальмувати поступово"
        );
    }

    #[test]
    fn returns_to_base_speed_but_never_faster() {
        let l = Limiter::new();
        l.reserve("etherscan", BASE);
        l.observe("etherscan", BASE, registry::Limit::Transient); // сповільнились
        let slowed = l.current_gap("etherscan", BASE);

        // Довга серія успіхів повертає нас до базової швидкості — і НЕ ШВИДШЕ за неї.
        for _ in 0..1000 {
            l.observe("etherscan", BASE, registry::Limit::None);
        }
        let back = l.current_gap("etherscan", BASE);
        assert!(back < slowed, "після серії успіхів мусимо пришвидшитись");
        assert_eq!(back, BASE, "але ніколи не швидше за базову паузу");
    }

    #[test]
    fn spaces_starts_by_the_current_gap() {
        let l = Limiter::new();
        // Три поспіль зарезервовані слоти мусять розійтися рівно на gap, а не злитися в бурст.
        let w1 = l.reserve("etherscan", BASE);
        let w2 = l.reserve("etherscan", BASE);
        let w3 = l.reserve("etherscan", BASE);
        assert!(w1.is_zero(), "перший стартує негайно");
        assert!(w2 >= BASE - Duration::from_millis(5), "другий чекає ~gap, а не стартує разом");
        assert!(w3 >= BASE * 2 - Duration::from_millis(5), "третій чекає ~2×gap");
    }

    /// Обидва тіла — справжні, зловлені на живому проді. Etherscan відмовляє HTTP 200 + NOTOK
    /// (хто чекає лише на 429 — не побачить ліміту взагалі), LunarCrush — 429 зі словом "daily".
    #[test]
    fn recognises_the_refusals_providers_actually_send() {
        use registry::Limit;
        let es = br#"{"status":"0","message":"NOTOK","result":"Max calls per sec rate limit reached (3/sec)"}"#;
        assert_eq!(registry::classify_limit(200, es), Limit::Transient);

        let lc = br#"{"error":"Rate limit exceeded (daily)"}"#;
        assert_eq!(registry::classify_limit(429, lc), Limit::Daily, "добовий ліміт мусить бути ОКРЕМИМ випадком");

        // 429 без уточнення — вважаємо минущим: сповільнитись безпечно, а помилково оголосити
        // добову вичерпаність означало б на годину відступити там, де вистачило б секунди.
        assert_eq!(registry::classify_limit(429, b""), Limit::Transient);

        // Порожній результат — це ДАНІ («переказів немає»), а не ліміт. Сплутати їх означало б
        // гальмувати на кожному порожньому гаманці.
        assert_eq!(registry::classify_limit(200, br#"{"status":"0","message":"No transactions found","result":[]}"#), Limit::None);
        assert_eq!(registry::classify_limit(200, br#"{"jsonrpc":"2.0","id":1,"result":"0x1857834"}"#), Limit::None);
    }
}

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

fn read(path: &str) -> std::io::Result<Vec<u8>> {
    std::fs::read(path)
}

static START: OnceLock<Instant> = OnceLock::new();

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    START.get_or_init(Instant::now);
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let id = env_or("SATELLITE_ID", "satellite");
    let addr = env_or("SATELLITE_ADDR", "0.0.0.0:8443").parse()?;
    let cert_dir = env_or("PKI_DIR", "/pki");

    // mTLS: сервер пред'являє свій сертифікат І ВИМАГАЄ клієнтський, підписаний нашим CA.
    // Без client_ca_root будь-хто, хто дотягнувся до порту, говорив би з сателітом.
    let identity = Identity::from_pem(
        read(&format!("{cert_dir}/{id}.crt")).map_err(|e| format!("{cert_dir}/{id}.crt: {e}"))?,
        read(&format!("{cert_dir}/{id}.key")).map_err(|e| format!("{cert_dir}/{id}.key: {e}"))?,
    );
    let ca = Certificate::from_pem(read(&format!("{cert_dir}/ca.crt")).map_err(|e| format!("{cert_dir}/ca.crt: {e}"))?);
    let tls = ServerTlsConfig::new().identity(identity).client_ca_root(ca);

    let configured: Vec<&str> =
        registry::PROVIDERS.iter().filter(|p| p.key().is_some()).map(|p| p.name).collect();
    let missing: Vec<&str> =
        registry::PROVIDERS.iter().filter(|p| p.key().is_none()).map(|p| p.name).collect();
    tracing::info!("[{id}] слухає {addr} (mTLS, клієнтський сертифікат ОБОВ'ЯЗКОВИЙ)");
    tracing::info!("[{id}] провайдери з ключем: {configured:?}");
    let share = registry::quota_share();
    if share > 1 {
        // Це не деталь — це наша частка чужої квоти. Якщо число не збігається з реальною
        // кількістю сателітів на акаунті, ми або б'ємо понад ліміт, або даремно себе гальмуємо.
        tracing::info!(
            "[{id}] спільна квота: ділимо акаунт на {share} сателітів → пауза {}× (etherscan: {} мс)",
            share,
            registry::effective_gap(registry::find("etherscan").unwrap()).as_millis()
        );
    } else {
        tracing::warn!("[{id}] SATELLITE_QUOTA_SHARE=1: вважаю, що квота провайдерів належить МЕНІ одному. Якщо ключі спільні з іншим сателітом — ми разом б'ємо понад ліміт");
    }
    if !missing.is_empty() {
        tracing::warn!("[{id}] БЕЗ ключа (Fetch віддасть виразну помилку, не тишу): {missing:?}");
    }

    let sat = Arc::new(Sat {
        id: id.clone(),
        http: reqwest::Client::builder()
            // Таймаут покриває і завантаження тіла, а тіло буває на 8+ МіБ (txlist на 10 000
            // записів). На 25 с такий виклик не встигав докачатися й повертався помилкою — тобто
            // найдовша історія, заради якої все й робиться, ламалася найпершою. connect_timeout
            // лишається коротким: мертвий провайдер має відпадати швидко, а не займати хвилину.
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(60))
            .user_agent("wallet-radar-satellite/1")
            .build()?,
        limiter: Limiter::new(),
        calls: Mutex::new(HashMap::new()),
    });

    Server::builder()
        .tls_config(tls)?
        .add_service(UpstreamServer::new(sat).max_encoding_message_size(MAX_MSG))
        .serve_with_shutdown(addr, async {
            tokio::signal::ctrl_c().await.ok();
            tracing::info!("зупинка");
        })
        .await?;
    Ok(())
}
