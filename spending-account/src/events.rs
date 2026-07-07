//! `EVENT_JSON` log emission for all state changes.
//!
//! Each state change has its own named function here; call sites emit through
//! them rather than constructing event names and JSON payloads inline.

use near_sdk::{env, AccountId, NearToken};

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

pub(crate) fn transfer(
    receiver_id: &AccountId,
    amount: NearToken,
    spent: NearToken,
    period_index: u64,
) {
    emit_event(
        "transfer",
        serde_json::json!({
            "receiver_id": receiver_id,
            "amount": amount,
            "spent": spent,
            "period_index": period_index,
        }),
    );
}

pub(crate) fn transfer_failed(receiver_id: &AccountId, amount: NearToken, period_index: u64) {
    emit_event(
        "transfer_failed",
        serde_json::json!({
            "receiver_id": receiver_id,
            "amount": amount,
            "period_index": period_index,
        }),
    );
}
