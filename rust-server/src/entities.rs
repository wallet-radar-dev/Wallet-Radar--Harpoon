// Мітки бірж та інституційних гаманців. Таблиця з 46 рядків переїхала у SQLite
// (`wallets`), разом із 7 адресами з wr-dict::known_exchange(), які раніше були
// окремим і неузгодженим переліком.
//
// УВАГА на семантику. Ця функція відповідає на питання «чи це інфраструктура,
// яку треба виключити з кандидатів», а не «чи знаємо ми цю адресу». У базі тепер
// лежать і гаманці ПІД НАГЛЯДОМ — якби `entity_of` повертала Some і для них,
// система виключила б із аналізу рівно ті адреси, заради яких існує. Тому
// фільтр за видом, а не просто «є запис».

use wr_store::snapshot;

pub struct Entity {
    pub name: String,
    #[allow(dead_code)] // читається, щойно допортують маршрути з мітками сутностей
    pub kind: String,
}

/// Види, що означають «інфраструктура, не приватний оператор».
/// `eoa` і `unknown` сюди НЕ входять: це якраз спостережувані гаманці.
const INFRA_KINDS: [&str; 6] = ["cex", "bridge", "defi", "issuer", "burn", "pool"];

/// server/entities.js:58 — `ENTITIES[addr.toLowerCase()] || null`
pub fn entity_of(addr: &str) -> Option<Entity> {
    let s = snapshot();
    let a = addr.to_lowercase();
    let kind = s.wallet_kind("eth", &a)?;
    if !INFRA_KINDS.contains(&kind) {
        return None;
    }
    let name = s.wallet_label("eth", &a).unwrap_or(kind).to_string();
    Some(Entity { name, kind: kind.to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watched_wallets_are_not_treated_as_infrastructure() {
        // Головна регресія, якої треба уникнути: концентратор мережі під наглядом
        // лежить у тій самій таблиці, що й біржі. Якщо його визнати інфраструктурою,
        // він випаде з усіх кандидатів — тихо й без жодної помилки в логах.
        if std::env::var("WR_DB").is_err() {
            return; // без сховища перевіряти нічого
        }
        // Адресу беремо З БАЗИ, а не з літерала: цей тест колись сам повернув
        // у репозиторій гаманець під наглядом, який ми звідти прибирали.
        let Ok(c) = wr_store::open() else { return };
        let concentrator: String = match c.query_row(
            "SELECT address FROM wallet_roles WHERE role='concentrator' LIMIT 1",
            [],
            |r| r.get(0),
        ) {
            Ok(a) => a,
            Err(_) => return, // немає такої ролі — перевіряти нічого
        };
        assert!(
            entity_of(&concentrator).is_none(),
            "гаманець під наглядом визнано інфраструктурою — його виключать з аналізу"
        );
    }

    #[test]
    fn exchange_is_recognised() {
        if std::env::var("WR_DB").is_err() {
            return;
        }
        let e = entity_of("0x28c6c06298d514db089934071355e5743bf21d60");
        assert_eq!(e.map(|e| e.name).as_deref(), Some("Binance 14"));
    }
}
