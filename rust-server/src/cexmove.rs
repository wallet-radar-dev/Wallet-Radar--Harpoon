//! `/api/move-check` — чи це справжній рух коштів, чи біржа перекладає з кишені в кишеню.
//!
//! НАЙПОШИРЕНІША ХИБА ЖАНРУ. Стрічки китових алертів (Whale Alert і всі, хто його
//! переказує) подають будь-який великий переказ як подію ринку. Насправді значна
//! частина таких переказів — біржа пересуває власні монети між власними гаманцями:
//! гарячий у холодний, ребалансування субгаманців, зміна кастодіана. Найгучніший
//! приклад: перекази XRP на $165.9 млн у лютому 2026, подані як китовий рух, виявились
//! ребалансуванням між субгаманцями однієї біржі.
//!
//! Спільнота виробила народне правило «якщо обидві адреси однієї біржі — викинь», але
//! жоден інструмент не продає це як функцію. Тут воно реалізоване й, головне,
//! ПОЯСНЕНЕ: у відповіді видно, які мітки спрацювали, тож вердикт можна перевірити.
//!
//! МЕЖА ЧЕСНОСТІ. Вердикт спирається на набір публічних міток, а він неповний за
//! означенням: адреса без мітки — це «не знаємо», а не «не біржа». Тому стан
//! `unlabeled` існує окремо й ніколи не видається за «справжній рух».

use crate::state::Ctx;
use axum::{extract::{Query, State}, response::IntoResponse, Json};
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Move {
    /// Обидві адреси — та сама біржа. Це НЕ подія ринку.
    InternalReshuffle,
    /// Різні біржі: монети справді змінюють майданчик.
    BetweenExchanges,
    /// Занесли на біржу — потенційний продаж.
    DepositToExchange,
    /// Забрали з біржі — потенційне виведення в холод.
    WithdrawalFromExchange,
    /// Обидві адреси не біржові й обидві відомі.
    OnChain,
    /// Хоча б одна адреса без мітки — сказати нічого не можемо.
    Unlabeled,
}

impl Move {
    pub fn key(self) -> &'static str {
        match self {
            Move::InternalReshuffle => "internal-reshuffle",
            Move::BetweenExchanges => "between-exchanges",
            Move::DepositToExchange => "deposit-to-exchange",
            Move::WithdrawalFromExchange => "withdrawal-from-exchange",
            Move::OnChain => "on-chain",
            Move::Unlabeled => "unlabeled",
        }
    }

    /// Чи вважати це подією ринку. `Unlabeled` свідомо НЕ «подія»: невідоме не має
    /// потрапляти в стрічку як підтверджений рух.
    pub fn is_market_event(self) -> bool {
        matches!(self, Move::BetweenExchanges | Move::DepositToExchange
            | Move::WithdrawalFromExchange | Move::OnChain)
    }

    pub fn explain(self) -> &'static str {
        match self {
            Move::InternalReshuffle => "Обидві адреси належать одній біржі — це перекладання власних монет, а не рух ринку.",
            Move::BetweenExchanges => "Переказ між різними біржами.",
            Move::DepositToExchange => "Занесення на біржу — монети стають доступними до продажу.",
            Move::WithdrawalFromExchange => "Виведення з біржі — монети йдуть із торгів.",
            Move::OnChain => "Обидві адреси не біржові: рух усередині ланцюга.",
            Move::Unlabeled => "Принаймні одна адреса без мітки — стверджувати нічого не можна.",
        }
    }
}

/// Ядро класифікації. Приймає вже знайдені біржі (або їх відсутність) і чи адреси
/// взагалі відомі набору міток.
pub fn classify(
    from_ex: Option<&str>, to_ex: Option<&str>, from_known: bool, to_known: bool,
) -> Move {
    match (from_ex, to_ex) {
        (Some(a), Some(b)) => {
            if a.eq_ignore_ascii_case(b) { Move::InternalReshuffle } else { Move::BetweenExchanges }
        }
        (None, Some(_)) => if from_known { Move::DepositToExchange } else { Move::Unlabeled },
        (Some(_), None) => if to_known { Move::WithdrawalFromExchange } else { Move::Unlabeled },
        (None, None) => if from_known && to_known { Move::OnChain } else { Move::Unlabeled },
    }
}

#[derive(Deserialize)]
pub struct MoveQuery {
    from: Option<String>,
    to: Option<String>,
}

pub async fn route(State(s): State<Ctx>, Query(q): Query<MoveQuery>) -> impl IntoResponse {
    let from = q.from.unwrap_or_default().trim().to_string();
    let to = q.to.unwrap_or_default().trim().to_string();
    if from.is_empty() || to.is_empty() {
        return Json(json!({"error": "потрібні обидві адреси: ?from=0x…&to=0x…"})).into_response();
    }

    let from_ex = s.labels.exchange_of(&from);
    let to_ex = s.labels.exchange_of(&to);
    let from_name = s.labels.label_name(&from);
    let to_name = s.labels.label_name(&to);
    let verdict = classify(
        from_ex.as_deref(), to_ex.as_deref(),
        from_name.is_some() || from_ex.is_some(),
        to_name.is_some() || to_ex.is_some(),
    );

    Json(json!({
        "from": {"address": from, "label": from_name, "exchange": from_ex},
        "to": {"address": to, "label": to_name, "exchange": to_ex},
        "verdict": verdict.key(),
        "isMarketEvent": verdict.is_market_event(),
        "explain": verdict.explain(),
        // Межа методу стоїть у самій відповіді, а не у виносці документації.
        "caveat": "Вердикт спирається на публічний набір міток. Адреса без мітки означає «не знаємо», а не «не біржа».",
    })).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Головний тест модуля: те, заради чого він існує.
    #[test]
    fn same_exchange_is_not_a_market_event() {
        let v = classify(Some("binance"), Some("binance"), true, true);
        assert_eq!(v, Move::InternalReshuffle);
        assert!(!v.is_market_event(), "перекладання всередині біржі не є подією ринку");
    }

    #[test]
    fn different_exchanges_is_a_real_move() {
        let v = classify(Some("binance"), Some("kraken"), true, true);
        assert_eq!(v, Move::BetweenExchanges);
        assert!(v.is_market_event());
    }

    #[test]
    fn deposit_and_withdrawal_are_distinguished() {
        assert_eq!(classify(None, Some("okx"), true, true), Move::DepositToExchange);
        assert_eq!(classify(Some("okx"), None, true, true), Move::WithdrawalFromExchange);
    }

    /// Невідоме не має видаватись за підтверджений рух — ані в той, ані в інший бік.
    #[test]
    fn unlabeled_is_never_a_confirmed_event() {
        let v = classify(None, Some("binance"), false, true);
        assert_eq!(v, Move::Unlabeled);
        assert!(!v.is_market_event());
        let v2 = classify(None, None, false, false);
        assert_eq!(v2, Move::Unlabeled);
        assert!(!v2.is_market_event());
    }

    #[test]
    fn case_does_not_break_same_exchange_detection() {
        assert_eq!(classify(Some("Binance"), Some("binance"), true, true), Move::InternalReshuffle);
    }
}
