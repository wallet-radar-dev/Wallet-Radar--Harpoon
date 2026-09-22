// Port of the server/index.js:94-173 gateway middleware stack: IP abuse-ban,
// HTTP Basic auth with two roles (admin/viewer) + brute-force lockout, and a
// per-IP fixed-window rate limit for viewers. All decision logic is pure over an
// injected `now` (epoch-ms) so it is unit-testable; the axum wiring + real clock
// live in main.rs. State is in-memory (resets on restart), exactly like Node.

use std::collections::HashMap;
use std::sync::Mutex;

pub struct AuthConfig {
    pub app_pass: String,
    pub admin_pass: String,
    pub rate_window_ms: i64,
    pub rate_max: i64,
    pub ban_strikes: i64,
    pub ban_ms: i64,
    pub strike_window_ms: i64,
}

impl AuthConfig {
    /// Read from env, matching index.js defaults.
    pub fn from_env() -> Self {
        let num = |k: &str, d: i64| std::env::var(k).ok().and_then(|v| v.parse::<i64>().ok()).filter(|n| *n != 0).unwrap_or(d);
        AuthConfig {
            app_pass: std::env::var("APP_PASSWORD").unwrap_or_default(),
            admin_pass: std::env::var("ADMIN_PASSWORD").unwrap_or_default(),
            rate_window_ms: num("RATE_LIMIT_WINDOW_MS", 60000),
            rate_max: num("RATE_LIMIT_MAX", 240),
            ban_strikes: num("RATE_BAN_STRIKES", 20),
            ban_ms: num("RATE_BAN_MS", 10 * 60 * 1000),
            strike_window_ms: 5 * 60 * 1000,
        }
    }
    pub fn no_passwords(&self) -> bool {
        self.app_pass.is_empty() && self.admin_pass.is_empty()
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Role {
    Admin,
    Viewer,
}

#[derive(PartialEq, Debug)]
pub enum AuthOutcome {
    Locked,       // 429 — brute-force lockout in effect
    Unauthorized, // 401 — missing/wrong password (this attempt recorded)
    Ok(Role),
}

pub struct RateResult {
    pub limit: i64,
    pub remaining: i64,
    pub reset: i64, // seconds
    pub allowed: bool,
}

#[derive(Default)]
struct Fail {
    n: i64,
    until: i64,
}
#[derive(Default)]
struct Ban {
    strikes: i64,
    first_strike_at: i64,
    banned_until: i64,
}
struct Hit {
    count: i64,
    reset_at: i64,
}

pub struct AuthState {
    /// token → (роль, час протухання, epoch-ms). Живе в памʼяті:
    /// перезапуск сервера розлогінює всіх — це прийнятно й навіть бажано.
    sessions: Mutex<HashMap<String, (Role, i64)>>,
    pub cfg: AuthConfig,
    auth_fails: Mutex<HashMap<String, Fail>>,
    abuse_bans: Mutex<HashMap<String, Ban>>,
    rate_hits: Mutex<HashMap<String, Hit>>,
}

impl AuthState {
    pub fn new(cfg: AuthConfig) -> Self {
        AuthState {
            cfg,
            auth_fails: Mutex::new(HashMap::new()),
            sessions: Mutex::new(HashMap::new()),
            abuse_bans: Mutex::new(HashMap::new()),
            rate_hits: Mutex::new(HashMap::new()),
        }
    }

    /// index.js `bannedUntilMs` — 0 if not currently banned.
    pub fn banned_until(&self, ip: &str, now: i64) -> i64 {
        let bans = self.abuse_bans.lock().unwrap();
        bans.get(ip).map(|r| if r.banned_until > now { r.banned_until } else { 0 }).unwrap_or(0)
    }

    /// index.js `recordAbuseStrike` — escalate a 429 into a temporary IP ban.
    pub fn record_abuse_strike(&self, ip: &str, now: i64) {
        let mut bans = self.abuse_bans.lock().unwrap();
        let mut r = bans.remove(ip).unwrap_or(Ban { strikes: 0, first_strike_at: now, banned_until: 0 });
        if now - r.first_strike_at > self.cfg.strike_window_ms {
            r = Ban { strikes: 0, first_strike_at: now, banned_until: 0 };
        }
        r.strikes += 1;
        if r.strikes >= self.cfg.ban_strikes {
            r.banned_until = now + self.cfg.ban_ms;
            r.strikes = 0;
            r.first_strike_at = now;
        }
        bans.insert(ip.to_string(), r);
        if bans.len() > 5000 {
            bans.retain(|_, v| !(v.banned_until < now && now - v.first_strike_at > self.cfg.strike_window_ms));
        }
    }

    /// index.js:130-148 auth middleware body. `auth_header` is the raw
    /// Authorization header value. Caller handles the no-password local mode.
    /// Тривалість сесії. 12 годин: робочий день без повторного входу,
    /// але не «назавжди» на чужому компʼютері.
    pub const SESSION_MS: i64 = 12 * 60 * 60 * 1000;

    /// Перевірити пароль БЕЗ запису невдалої спроби — форма входу має власний
    /// облік, інакше один друк помилки блокував би і Basic-клієнтів.
    pub fn role_for_password(&self, pass: &str) -> Option<Role> {
        if !self.cfg.admin_pass.is_empty() && pass == self.cfg.admin_pass {
            return Some(Role::Admin);
        }
        if !self.cfg.app_pass.is_empty() && pass == self.cfg.app_pass {
            return Some(Role::Viewer);
        }
        None
    }

    /// Створити сесію й повернути токен для cookie.
    pub fn create_session(&self, role: Role, now: i64, token: String) -> String {
        let mut ss = self.sessions.lock().unwrap();
        // прибрати протухлі, щоб мапа не росла нескінченно
        ss.retain(|_, (_, exp)| *exp > now);
        ss.insert(token.clone(), (role, now + Self::SESSION_MS));
        token
    }

    /// Роль за cookie-заголовком, якщо сесія жива.
    pub fn check_session(&self, cookie_header: &str, now: i64) -> Option<Role> {
        let token = cookie_header
            .split(';')
            .map(|c| c.trim())
            .find_map(|c| c.strip_prefix("wr_session="))?;
        let ss = self.sessions.lock().unwrap();
        match ss.get(token) {
            Some((role, exp)) if *exp > now => Some(*role),
            _ => None,
        }
    }

    /// Завершити сесію (вихід).
    pub fn drop_session(&self, cookie_header: &str) {
        if let Some(token) = cookie_header
            .split(';')
            .map(|c| c.trim())
            .find_map(|c| c.strip_prefix("wr_session="))
        {
            self.sessions.lock().unwrap().remove(token);
        }
    }

    pub fn check_auth(&self, ip: &str, auth_header: &str, now: i64) -> AuthOutcome {
        let mut fails = self.auth_fails.lock().unwrap();
        if let Some(rec) = fails.get(ip) {
            if rec.until > now {
                return AuthOutcome::Locked;
            }
        }
        let b64 = auth_header.strip_prefix("Basic ").unwrap_or("");
        let decoded = base64_decode(b64);
        let text = String::from_utf8_lossy(&decoded);
        let pass = text.splitn(2, ':').nth(1).unwrap_or("");
        if !self.cfg.admin_pass.is_empty() && pass == self.cfg.admin_pass {
            fails.remove(ip);
            return AuthOutcome::Ok(Role::Admin);
        }
        if !self.cfg.app_pass.is_empty() && pass == self.cfg.app_pass {
            fails.remove(ip);
            return AuthOutcome::Ok(Role::Viewer);
        }
        // failed attempt
        let f = fails.entry(ip.to_string()).or_default();
        if f.until != 0 && f.until <= now {
            *f = Fail::default();
        }
        f.n += 1;
        if f.n >= 12 {
            f.until = now + 3 * 60 * 1000;
            f.n = 0;
        }
        if fails.len() > 2000 {
            fails.retain(|_, v| v.until >= now);
        }
        AuthOutcome::Unauthorized
    }

    /// index.js:160-172 rate-limit middleware (viewers only). Always returns the
    /// RateLimit-* header values; `allowed=false` means 429 (caller records a
    /// strike). Mirrors the fixed-window counter.
    pub fn check_rate(&self, ip: &str, now: i64) -> RateResult {
        let mut hits = self.rate_hits.lock().unwrap();
        let rec = hits.entry(ip.to_string()).or_insert(Hit { count: 0, reset_at: 0 });
        if rec.reset_at == 0 || now >= rec.reset_at {
            *rec = Hit { count: 0, reset_at: now + self.cfg.rate_window_ms };
        }
        rec.count += 1;
        let retry_sec = ((rec.reset_at - now) as f64 / 1000.0).ceil() as i64;
        let remaining = (self.cfg.rate_max - rec.count).max(0);
        let over = rec.count > self.cfg.rate_max;
        let count_is_new = hits.len() > 5000;
        if count_is_new {
            hits.retain(|_, v| now < v.reset_at);
        }
        RateResult { limit: self.cfg.rate_max, remaining, reset: retry_sec, allowed: !over }
    }
}

/// Minimal standard-alphabet base64 decoder (credentials only) — avoids a new
/// crate dep. Skips padding and any non-alphabet byte, like a lenient decoder.
fn base64_decode(s: &str) -> Vec<u8> {
    fn val(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let mut out = Vec::new();
    let mut buf = 0u32;
    let mut bits = 0u32;
    for &c in s.as_bytes() {
        if let Some(v) = val(c) {
            buf = (buf << 6) | v as u32;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                out.push((buf >> bits) as u8);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> AuthState {
        AuthState::new(AuthConfig {
            app_pass: "viewpw".into(),
            admin_pass: "adminpw".into(),
            rate_window_ms: 60000,
            rate_max: 3,
            ban_strikes: 2,
            ban_ms: 600000,
            strike_window_ms: 300000,
        })
    }

    fn basic(user: &str, pass: &str) -> String {
        // encode user:pass to base64 (standard) for the test header
        let raw = format!("{user}:{pass}");
        let bytes = raw.as_bytes();
        const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::from("Basic ");
        for chunk in bytes.chunks(3) {
            let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
            out.push(A[(b[0] >> 2) as usize] as char);
            out.push(A[(((b[0] & 3) << 4) | (b[1] >> 4)) as usize] as char);
            out.push(if chunk.len() > 1 { A[(((b[1] & 15) << 2) | (b[2] >> 6)) as usize] as char } else { '=' });
            out.push(if chunk.len() > 2 { A[(b[2] & 63) as usize] as char } else { '=' });
        }
        out
    }

    #[test]
    fn base64_roundtrip() {
        assert_eq!(base64_decode("YWRtaW46YWRtaW5wdw=="), b"admin:adminpw");
        assert_eq!(String::from_utf8_lossy(&base64_decode(&basic("admin", "adminpw")[6..])), "admin:adminpw");
    }

    #[test]
    fn admin_and_viewer_ok() {
        let s = state();
        assert_eq!(s.check_auth("ip1", &basic("admin", "adminpw"), 0), AuthOutcome::Ok(Role::Admin));
        assert_eq!(s.check_auth("ip1", &basic("x", "viewpw"), 0), AuthOutcome::Ok(Role::Viewer));
    }

    #[test]
    fn wrong_password_locks_after_12() {
        let s = state();
        for _ in 0..11 {
            assert_eq!(s.check_auth("ip2", &basic("x", "nope"), 0), AuthOutcome::Unauthorized);
        }
        // 12th failure sets the lockout; the NEXT call sees until>now → Locked
        assert_eq!(s.check_auth("ip2", &basic("x", "nope"), 0), AuthOutcome::Unauthorized);
        assert_eq!(s.check_auth("ip2", &basic("x", "nope"), 1000), AuthOutcome::Locked);
        // correct password during lockout is still Locked
        assert_eq!(s.check_auth("ip2", &basic("admin", "adminpw"), 1000), AuthOutcome::Locked);
        // after the 3-min block expires, a correct password clears it
        assert_eq!(s.check_auth("ip2", &basic("admin", "adminpw"), 3 * 60 * 1000 + 1), AuthOutcome::Ok(Role::Admin));
    }

    #[test]
    fn rate_limit_and_abuse_ban() {
        let s = state(); // rate_max=3, ban_strikes=2
        for i in 1..=3 {
            let r = s.check_rate("ip3", 0);
            assert!(r.allowed, "req {i} allowed");
            assert_eq!(r.remaining, 3 - i);
        }
        // 4th exceeds → not allowed; caller records a strike
        let r = s.check_rate("ip3", 0);
        assert!(!r.allowed);
        s.record_abuse_strike("ip3", 0);
        assert_eq!(s.banned_until("ip3", 0), 0); // 1 strike < 2, not yet banned
        let r = s.check_rate("ip3", 0);
        assert!(!r.allowed);
        s.record_abuse_strike("ip3", 0); // 2nd strike → ban
        assert_eq!(s.banned_until("ip3", 0), 600000);
        assert_eq!(s.banned_until("ip3", 600001), 0); // ban expired
    }

    #[test]
    fn rate_window_resets() {
        let s = state();
        for _ in 0..3 {
            let _ = s.check_rate("ip4", 0);
        }
        assert!(!s.check_rate("ip4", 0).allowed);
        // new window
        assert!(s.check_rate("ip4", 60000).allowed);
    }
}
