//! systemd sd_notify: READY=1 на старті (Type=notify) + liveness-watchdog
//! (WatchdogSec у юніті). Чистий Rust — крейт `sd-notify` пише в NOTIFY_SOCKET
//! напряму, без libsystemd. Поза systemd (немає NOTIFY_SOCKET) усі виклики —
//! безпечний no-op, тож локальний `cargo run` не змінюється.
//!
//! Що ловить: повне зависання рантайму. Keep-alive — це tokio-задача; якщо
//! рантайм заклинить (дедлок, стоп планувальника), пінги припиняються і systemd
//! за WatchdogSec вбʼє й перезапустить процес. НЕ ловить одного мертвого воркера
//! при живому рантаймі — це навмисно лишене осторонь, щоб не було хибних вбивств.

//! Поза Linux (ноут під Windows, коли ВМ недоступна) systemd не існує — там `arm()`
//! нічого не робить, і це чесніше за емуляцію: сторожа, якого нема кому перезапускати,
//! бути не повинно.

#[cfg(unix)]
mod imp {
use sd_notify::NotifyState;
use std::time::Duration;

/// READY=1 і, якщо юніт задав WatchdogSec, keep-alive. Кликати один раз після bind.
pub fn arm() {
    if let Err(e) = sd_notify::notify(false, &[NotifyState::Ready]) {
        tracing::debug!("[systemd] READY notify пропущено: {e}");
    }
    let mut usec: u64 = 0;
    if sd_notify::watchdog_enabled(false, &mut usec) && usec > 0 {
        let period = Duration::from_micros(usec / 2);
        tracing::info!(
            "[systemd] watchdog увімкнено: WatchdogSec={}s, пінг кожні {}s",
            usec / 1_000_000,
            period.as_secs()
        );
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(period);
            loop {
                tick.tick().await;
                if let Err(e) = sd_notify::notify(false, &[NotifyState::Watchdog]) {
                    tracing::warn!("[systemd] watchdog-пінг не вдався: {e}");
                }
            }
        });
    } else {
        tracing::info!("[systemd] watchdog вимкнено (немає WatchdogSec або не під systemd)");
    }
}
}

#[cfg(not(unix))]
mod imp {
    pub fn arm() {
        tracing::info!("[systemd] watchdog недоступний поза Linux — пропущено");
    }
}

pub use imp::arm;
