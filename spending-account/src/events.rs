//! `EVENT_JSON` log emission. Call sites emit through the named functions here
//! rather than constructing event names and JSON payloads inline.

use near_sdk::json_types::U128;
use near_sdk::{AccountId, NearToken, env};

const EVENT_STANDARD: &str = "spending-account";
const EVENT_VERSION: &str = "1.0.0";

fn emit_event(event: &str, data: serde_json::Value) {
    env::log_str(&format!(
        "EVENT_JSON:{}",
        serde_json::json!({
            "standard": EVENT_STANDARD,
            "version": EVENT_VERSION,
            "event": event,
            "data": [data],
        })
    ));
}

pub(crate) fn transfer(receiver_id: &AccountId, amount: NearToken, limit: NearToken) {
    emit_event(
        "transfer",
        serde_json::json!({
            "receiver_id": receiver_id,
            "amount": amount,
            "limit": limit,
        }),
    );
}

pub(crate) fn transfer_failed(receiver_id: &AccountId, amount: NearToken) {
    emit_event(
        "transfer_failed",
        serde_json::json!({
            "receiver_id": receiver_id,
            "amount": amount,
        }),
    );
}

pub(crate) fn ft_transfer(
    receiver_id: &AccountId,
    token_id: &AccountId,
    amount: U128,
    limit: U128,
) {
    emit_event(
        "ft_transfer",
        serde_json::json!({
            "receiver_id": receiver_id,
            "token_id": token_id,
            "amount": amount,
            "limit": limit,
        }),
    );
}

pub(crate) fn ft_transfer_failed(receiver_id: &AccountId, token_id: &AccountId, amount: U128) {
    emit_event(
        "ft_transfer_failed",
        serde_json::json!({
            "receiver_id": receiver_id,
            "token_id": token_id,
            "amount": amount,
        }),
    );
}
