//! Парність Node ↔ Rust: обидва сервери піднімають один датасет, і кожен маршрут мусить
//! віддавати БАЙТ-У-БАЙТ те саме. Порт `tools/parity_check.py` (Python з проєкту прибрано).
//!
//! Запуск: Node на 3000, Rust на 3001 — і `cargo run --release --bin parity_check`.
//! Порти можна перевизначити: `parity_check <node_port> <rust_port>`.
//!
//! Пароль. Сервер за Basic-авторизацією, і саме тут Python-версія була вже не робочою: вона
//! ходила без заголовка й діставала 401 на КОЖНОМУ маршруті — тобто «розбіжностей немає» вона
//! сказати не могла в принципі. Пароль береться з ADMIN_PASSWORD/APP_PASSWORD оточення (як і на
//! самому сервері); якщо їх немає, сервер у локальному режимі пускає без пароля, і заголовок
//! просто не потрібен.

use serde_json::Value;

/// Маршрути, які мусять збігатися. Порядок — як у списку, щоб вивід читався однаково між запусками.
const ROUTES: [&str; 14] = [
    "/api/config",
    "/api/clarity-tags",
    "/api/wallets",
    "/api/events",
    "/api/events?limit=7",
    "/api/whale-assets",
    "/api/cross-asset",
    "/api/capital-groups",
    "/api/asset-forensics",
    "/api/whale-clusters",
    "/api/discovered-groups",
    "/api/featured-groups",
    "/api/health",
    "/api/version",
];

/// Поля, які РІЗНЯТЬСЯ законно: вони описують сам рантайм, а не дані.
fn ignored(path: &str) -> &'static [&'static str] {
    match path.split('?').next().unwrap_or(path) {
        "/api/health" => &["uptime"],
        "/api/version" => &["node", "runtime"],
        _ => &[],
    }
}

/// Викидає з відповіді поля, розбіжність яких нічого не означає.
fn scrub(path: &str, mut v: Value) -> Value {
    let ig = ignored(path);
    if let Some(o) = v.as_object_mut() {
        for k in ig {
            o.remove(*k);
        }
    }
    v
}

/// Basic-заголовок із пароля. Своя base64, щоб не тягнути залежність заради 4 рядків
/// (у auth.rs із тієї ж причини живе власний декодер).
fn basic_auth() -> Option<String> {
    let pass = std::env::var("ADMIN_PASSWORD")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("APP_PASSWORD").ok().filter(|s| !s.is_empty()))?;
    Some(format!("Basic {}", base64_encode(format!("parity:{pass}").as_bytes())))
}

fn base64_encode(data: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(A[(n >> 18 & 63) as usize] as char);
        out.push(A[(n >> 12 & 63) as usize] as char);
        out.push(if c.len() > 1 { A[(n >> 6 & 63) as usize] as char } else { '=' });
        out.push(if c.len() > 2 { A[(n & 63) as usize] as char } else { '=' });
    }
    out
}

async fn get(client: &reqwest::Client, port: u16, path: &str) -> anyhow::Result<(u16, Value)> {
    let mut rq = client.get(format!("http://127.0.0.1:{port}{path}"));
    if let Some(a) = basic_auth() {
        rq = rq.header("authorization", a);
    }
    let r = rq.send().await?;
    let status = r.status().as_u16();
    // Тіло парситься завжди, навіть на 4xx: сервер і в помилці віддає JSON, і його теж треба
    // звірити — інакше два різні тексти помилки прикинулися б однаковими.
    let body: Value = serde_json::from_slice(&r.bytes().await?).unwrap_or(Value::Null);
    Ok((status, body))
}

/// `12345` → `12,345`: розмір відповіді має читатися очима, а не рахуватися пальцем.
fn group_digits(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn clip(v: &Value, n: usize) -> String {
    v.to_string().chars().take(n).collect()
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let node_port: u16 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(3000);
    let rust_port: u16 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(3001);

    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(20)).build()?;
    let (mut ok, mut fail) = (0u32, 0u32);

    for p in ROUTES {
        let (node, rust) = match (get(&client, node_port, p).await, get(&client, rust_port, p).await) {
            (Ok(n), Ok(r)) => (n, r),
            (Err(e), _) | (_, Err(e)) => {
                println!("  ERROR  {p}: {e}");
                fail += 1;
                continue;
            }
        };
        let (sn, sr) = (node.0, rust.0);
        if sn != sr {
            println!("  STATUS {p}: node={sn} rust={sr}");
            fail += 1;
            continue;
        }
        let (n, r) = (scrub(p, node.1), scrub(p, rust.1));
        if n == r {
            println!("  MATCH  {p}  ({} bytes)", group_digits(n.to_string().len()));
            ok += 1;
            continue;
        }

        fail += 1;
        println!("  DIFF   {p}");
        match (n.as_object(), r.as_object()) {
            // Пооб'єктна різниця, а не «дві простирадла JSON»: інакше знайти РОЗБІЖНЕ поле в
            // мегабайтній відповіді неможливо, і звіт стає марним саме там, де він потрібен.
            (Some(no), Some(ro)) => {
                let mut keys: Vec<&String> = no.keys().chain(ro.keys()).collect();
                keys.sort();
                keys.dedup();
                let missing = Value::String("<missing>".into());
                for k in keys {
                    let (a, b) = (no.get(k).unwrap_or(&missing), ro.get(k).unwrap_or(&missing));
                    if a != b {
                        println!("           .{k}: node={}  rust={}", clip(a, 90), clip(b, 90));
                    }
                }
            }
            _ => {
                println!("           node={}", clip(&n, 120));
                println!("           rust={}", clip(&r, 120));
            }
        }
    }

    println!("\n  {ok} match, {fail} differ, {} total", ROUTES.len());
    if fail > 0 {
        std::process::exit(1);
    }
    Ok(())
}
