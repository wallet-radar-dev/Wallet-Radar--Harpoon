//! Клієнт до сателітів: усі keyed-виклики (Etherscan, Moralis, LunarCrush, Arkham, Glassnode)
//! ідуть по gRPC поверх mTLS на сателіт, який і тримає ключ.
//!
//! СУВОРИЙ РЕЖИМ. Якщо `SATELLITE_ENDPOINTS` заданий, кор більше НЕ ходить у цих провайдерів
//! напряму — навіть якщо ключ випадково лишився в його оточенні. Це і є сенс ізоляції: інакше
//! «ключі тільки на сателітах» тихо перетворюється на «ключі ще й на сателітах».
//! Якщо змінна НЕ задана, кор працює як раніше (прямі виклики) — щоб увімкнення сателітів було
//! окремим свідомим кроком, а не побічним ефектом деплою.
//!
//! Формат: `SATELLITE_ENDPOINTS="satellite-a=https://127.0.0.1:8443,satellite-b=https://127.0.0.1:8444"`
//! Ім'я ліворуч — це ім'я, на яке виписаний сертифікат сателіта (SAN). Воно потрібне саме тому,
//! що ходимо ми на 127.0.0.1: без явного domain_name TLS відкинув би сертифікат як
//! «NotValidForName», і це виглядало б як загадковий збій мережі.

use serde_json::Value;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;
use std::time::Duration;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Identity};

pub mod pb {
    tonic::include_proto!("satellite.v1");
}
use pb::upstream_client::UpstreamClient;
use pb::FetchRequest;

struct Endpoint {
    name: String,
    channel: Channel,
}

/// Стеля на тіло відповіді провайдера. Дефолт tonic — 4 МіБ, і його вистачало рівно доти,
/// доки не трапився перший гаманець із довгою історією: txlist на 1000 рядків важить більше,
/// сателіт віддавав його чесно, а кор рвав з'єднання на декодуванні. Назовні це виглядало не
/// як помилка, а як гаманець без переказів — тобто рівно та сама тиха неправда, проти якої
/// написаний весь цей модуль.
const MAX_MSG: usize = 64 * 1024 * 1024;

/// Біржовий гарячий гаманець для проби «велике тіло відповіді».
/// Береться з бази; порожньо — значить сховище не завантажене, і проба
/// має про це сказати, а не тихо піти з порожньою адресою.
fn probe_hot_wallet() -> String {
    let Ok(c) = wr_store::open() else { return String::new() };
    let Ok(mut q) =
        c.prepare("SELECT address FROM wallets WHERE kind='cex' ORDER BY address LIMIT 1")
    else {
        return String::new();
    };
    q.query_row([], |r| r.get(0)).unwrap_or_default()
}

fn client(ep: &Endpoint) -> UpstreamClient<Channel> {
    UpstreamClient::new(ep.channel.clone()).max_decoding_message_size(MAX_MSG)
}

struct Pool {
    eps: Vec<Endpoint>,
    next: AtomicUsize,
}

fn pki_dir() -> String {
    std::env::var("PKI_DIR").unwrap_or_else(|_| "deploy/pki/out".to_string())
}

/// `None` = сателіти не налаштовані → кор працює у старому прямому режимі.
fn pool() -> Option<&'static Pool> {
    static P: OnceLock<Option<Pool>> = OnceLock::new();
    P.get_or_init(|| {
        let raw = std::env::var("SATELLITE_ENDPOINTS").ok().filter(|s| !s.is_empty())?;
        let dir = pki_dir();
        let ca = std::fs::read(format!("{dir}/ca.crt"))
            .map_err(|e| tracing::error!("[sat] немає {dir}/ca.crt: {e}"))
            .ok()?;
        let crt = std::fs::read(format!("{dir}/core.crt"))
            .map_err(|e| tracing::error!("[sat] немає {dir}/core.crt: {e}"))
            .ok()?;
        let key = std::fs::read(format!("{dir}/core.key"))
            .map_err(|e| tracing::error!("[sat] немає {dir}/core.key: {e}"))
            .ok()?;

        let mut eps = Vec::new();
        for part in raw.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            let (name, url) = match part.split_once('=') {
                Some((n, u)) => (n.trim().to_string(), u.trim().to_string()),
                None => {
                    tracing::error!("[sat] хибний запис {part:?} — очікую 'ім'я=https://хост:порт'");
                    continue;
                }
            };
            let tls = ClientTlsConfig::new()
                .ca_certificate(Certificate::from_pem(ca.clone()))
                .identity(Identity::from_pem(crt.clone(), key.clone()))
                // Ім'я з сертифіката, а не з URL — ходимо на IP, довіряємо імені.
                .domain_name(name.clone());
            let ch = match Channel::from_shared(url.clone())
                .map_err(|e| format!("{e}"))
                .and_then(|c| c.tls_config(tls).map_err(|e| format!("{e}")))
            {
                // Стеля виклику мусить бути ЗАВІДОМО більша за таймаут самого сателіта (60 с):
                // інакше кор уб'є з'єднання раніше, ніж сателіт докачає велике тіло, і повільна
                // відповідь виглядатиме як мертвий сателіт. Швидко відпадати — робота
                // connect_timeout, а не цієї стелі.
                Ok(c) => c.connect_timeout(Duration::from_secs(5)).timeout(Duration::from_secs(90)).connect_lazy(),
                Err(e) => {
                    tracing::error!("[sat] {name}: {e}");
                    continue;
                }
            };
            tracing::info!("[sat] сателіт {name} → {url}");
            eps.push(Endpoint { name, channel: ch });
        }
        if eps.is_empty() {
            tracing::error!("[sat] SATELLITE_ENDPOINTS заданий, але жодного робочого сателіта — keyed-виклики падатимуть (це навмисно, суворий режим)");
        }
        Some(Pool { eps, next: AtomicUsize::new(0) })
    })
    .as_ref()
}

/// Чи ввімкнений суворий режим: keyed-провайдери доступні ЛИШЕ через сателіти.
pub fn strict() -> bool {
    pool().is_some()
}

/// Один виклик провайдера через будь-який живий сателіт (round-robin, з переходом до
/// наступного, якщо цей лежить).
///
/// `Ok((status, body))` — відповідь провайдера як є. `Err` — жоден сателіт не відповів або
/// провайдер не налаштований. Порожній `Ok` НІКОЛИ не підміняє помилку: саме ця підміна
/// (порожній список замість «джерело недоступне») і робила дані тихо неправдивими.
pub async fn fetch(provider: &str, path: &str, query: &[(&str, String)]) -> anyhow::Result<(u16, Vec<u8>)> {
    let p = pool().ok_or_else(|| anyhow::anyhow!("сателіти не налаштовані"))?;
    if p.eps.is_empty() {
        anyhow::bail!("немає жодного сателіта");
    }
    let start = p.next.fetch_add(1, Ordering::Relaxed);
    let mut last_err = String::new();
    for i in 0..p.eps.len() {
        let ep = &p.eps[(start + i) % p.eps.len()];
        let req = FetchRequest {
            provider: provider.to_string(),
            path: path.to_string(),
            query: query.iter().map(|(k, v)| (k.to_string(), v.clone())).collect(),
        };
        let mut client = client(ep);
        match client.fetch(req).await {
            Ok(resp) => {
                let r = resp.into_inner();
                return Ok((r.status as u16, r.body));
            }
            Err(st) => {
                // failed_precondition = у цього сателіта немає ключа саме до цього провайдера.
                // Інший сателіт може його мати, тож пробуємо далі, а не здаємося.
                last_err = format!("{}: {} ({})", ep.name, st.message(), st.code());
                tracing::warn!("[sat] {last_err}");
            }
        }
    }
    anyhow::bail!("жоден сателіт не виконав {provider}{path}: {last_err}")
}

/// Зручна обгортка: відповідь як JSON. HTTP-статус провайдера НЕ ковтається — 4xx/5xx
/// повертаються помилкою, бо для викликача це збій джерела, а не порожні дані.
pub async fn fetch_json(provider: &str, path: &str, query: &[(&str, String)]) -> anyhow::Result<Value> {
    let (status, body) = fetch(provider, path, query).await?;
    if !(200..300).contains(&status) {
        anyhow::bail!("{provider}{path} → HTTP {status}");
    }
    Ok(serde_json::from_slice(&body)?)
}

/// Стан сателітів — для /api/workers-health: видно, хто живий і в кого який ключ.
pub async fn health() -> Vec<Value> {
    let Some(p) = pool() else { return vec![] };
    let mut out = Vec::new();
    for ep in &p.eps {
        let mut client = client(ep);
        match client.health(pb::HealthRequest {}).await {
            Ok(r) => {
                let r = r.into_inner();
                out.push(serde_json::json!({
                    "id": r.satellite_id,
                    "up": true,
                    "providers": r.providers.iter().map(|x| serde_json::json!({
                        "name": x.name, "configured": x.configured,
                        "minGapMs": x.min_gap_ms, "calls": x.calls_total,
                    })).collect::<Vec<_>>(),
                }));
            }
            Err(st) => out.push(serde_json::json!({
                "id": ep.name, "up": false, "error": st.message(),
            })),
        }
    }
    out
}

/// `rust-server --probe-satellites` — перевірка каналу кор↔сателіт БЕЗ підняття 20 воркерів.
///
/// Йде тим самим кодом, що й прод (той самий pool(), той самий TLS, той самий fetch), тому
/// зелений probe означає саме те, що написано, а не «тест пройшов».
///
/// Перевіряємо не лише щасливий шлях. Сателіт мусить ВІДМОВЛЯТИ на спробу вийти за межі
/// реєстру — інакше він відкритий проксі всередину нашої мережі, і це страшніше за відсутність
/// даних. Тому відмова на чужий хост тут — УСПІХ, а не збій.
pub async fn probe() -> anyhow::Result<()> {
    let Some(p) = pool() else {
        anyhow::bail!("SATELLITE_ENDPOINTS не заданий — кор у прямому режимі, перевіряти нічого");
    };
    println!("Сателітів у пулі: {}\n", p.eps.len());

    println!("── Health (mTLS-рукостискання + стан ключів)");
    let h = health().await;
    if h.is_empty() {
        anyhow::bail!("жоден сателіт не відповів");
    }
    let mut alive = 0;
    for s in &h {
        if s["up"] == serde_json::json!(true) {
            alive += 1;
            let provs: Vec<String> = s["providers"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|x| {
                            let mark = if x["configured"] == serde_json::json!(true) { "✓" } else { "✗" };
                            format!("{mark}{}", x["name"].as_str().unwrap_or("?"))
                        })
                        .collect()
                })
                .unwrap_or_default();
            println!("  ✓ {} живий — {}", s["id"].as_str().unwrap_or("?"), provs.join(" "));
        } else {
            println!("  ✗ {} НЕ ВІДПОВІВ: {}", s["id"].as_str().unwrap_or("?"), s["error"].as_str().unwrap_or(""));
        }
    }
    if alive == 0 {
        anyhow::bail!("жоден сателіт не піднявся");
    }

    println!("\n── Реальний виклик Etherscan через сателіт (ключ підставляє ВІН)");
    let q = [("chainid", "1".to_string()), ("module", "proxy".into()), ("action", "eth_blockNumber".into())];
    let (status, body) = fetch("etherscan", "/v2/api", &q).await?;
    let text = String::from_utf8_lossy(&body);
    let j: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let block = j["result"].as_str().unwrap_or("");
    if status != 200 || block.is_empty() {
        anyhow::bail!("Etherscan через сателіт: HTTP {status}, тіло {}", text.chars().take(200).collect::<String>());
    }
    let n = i64::from_str_radix(block.trim_start_matches("0x"), 16).unwrap_or(0);
    println!("  ✓ HTTP {status}, поточний блок Ethereum = {n} ({block})");
    println!("    Кор ключа НЕ бачив: у FetchRequest його немає й бути не може.");

    println!("\n── Межа безпеки: сателіт мусить відмовити (це УСПІХ)");
    let cases: [(&str, &str, &str); 3] = [
        ("невідомий провайдер", "evil", "/x"),
        ("втеча на чужий хост", "etherscan", "https://evil.example/x"),
        ("обхід угору", "etherscan", "/v2/../../etc/passwd"),
    ];
    for (label, prov, path) in cases {
        match fetch(prov, path, &[]).await {
            Ok((st, _)) => anyhow::bail!("ДІРА: {label} ({prov}{path}) пройшов, HTTP {st}"),
            Err(e) => {
                let msg = e.to_string();
                let tail = msg.rsplit(':').next().unwrap_or(&msg).trim().to_string();
                println!("  ✓ {label} — відмовлено ({tail})");
            }
        }
    }

    // Регресія, заради якої цей блок і існує: доки стеля була дефолтна (4 МіБ), кор рвав
    // з'єднання саме на найцінніших відповідях — довгій історії великого гаманця. Помилка
    // виглядала як «переказів немає», тобто як ДАНІ, а не як збій. Тому перевіряємо не
    // «канал живий», а «канал переживає тіло, більше за стару стелю».
    println!("\n── Велике тіло: txlist на 10 000 записів (стара стеля — 4 МіБ)");
    let big = [
        ("chainid", "1".to_string()),
        ("module", "account".into()),
        ("action", "txlist".into()),
        // Проба на великий обсяг: беремо будь-який біржовий гарячий гаманець
        // із бази — їхня історія свідомо величезна. Конкретна адреса значення
        // не має, тому й не зашита: важлива лише властивість «дуже довга історія».
        ("address", probe_hot_wallet()),
        ("startblock", "0".into()),
        ("endblock", "99999999".into()),
        ("page", "1".into()),
        ("offset", "10000".into()),
        ("sort", "desc".into()),
    ];
    let (status, body) = fetch("etherscan", "/v2/api", &big).await?;
    let mib = body.len() as f64 / (1024.0 * 1024.0);
    let j: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let rows = j["result"].as_array().map(|a| a.len()).unwrap_or(0);
    if status != 200 || rows == 0 {
        anyhow::bail!("txlist через сателіт: HTTP {status}, {rows} записів");
    }
    println!("  ✓ HTTP {status}, {rows} записів, {mib:.2} МіБ — прийнято цілим");
    if body.len() <= 4 * 1024 * 1024 {
        println!("  ⚠ тіло вийшло менше за 4 МіБ — стелю це НЕ перевірило (гаманець схуд?)");
    } else {
        println!("    Понад стару стелю: на дефолтному ліміті цей виклик обірвався б.");
    }

    println!("\nКанал кор↔сателіт справний.");
    Ok(())
}
