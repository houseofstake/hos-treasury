//! `EVENT_JSON` log emission for all state changes.
//!
//! Each state change has its own named function here; call sites emit through
//! them rather than constructing event names and JSON payloads inline.

use near_sdk::json_types::U64;
use near_sdk::{env, AccountId, NearToken};

const EVENT_STANDARD: &str = "dao-timelock";
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

pub(crate) fn schedule(
    request_id: u64,
    receiver_id: &AccountId,
    execute_after: U64,
    total_deposit: NearToken,
) {
    emit_scheduled("schedule", request_id, receiver_id, execute_after, total_deposit);
}

pub(crate) fn schedule_proposal(
    request_id: u64,
    receiver_id: &AccountId,
    execute_after: U64,
    total_deposit: NearToken,
) {
    emit_scheduled(
        "schedule_proposal",
        request_id,
        receiver_id,
        execute_after,
        total_deposit,
    );
}

/// Shared payload for the two request-scheduling events.
fn emit_scheduled(
    event: &str,
    request_id: u64,
    receiver_id: &AccountId,
    execute_after: U64,
    total_deposit: NearToken,
) {
    emit_event(
        event,
        serde_json::json!({
            "request_id": request_id,
            "receiver_id": receiver_id,
            "execute_after": execute_after,
            "total_deposit": total_deposit,
        }),
    );
}

pub(crate) fn execute(request_id: u64, receiver_id: &AccountId) {
    emit_event(
        "execute",
        serde_json::json!({
            "request_id": request_id,
            "receiver_id": receiver_id,
        }),
    );
}

pub(crate) fn approve_proposal(request_id: u64, proposal_id: u64) {
    emit_event(
        "approve_proposal",
        serde_json::json!({
            "request_id": request_id,
            "proposal_id": proposal_id,
        }),
    );
}

pub(crate) fn proposal_failed(request_id: u64, funder_id: &AccountId, refund: NearToken) {
    emit_event(
        "proposal_failed",
        serde_json::json!({
            "request_id": request_id,
            "funder_id": funder_id,
            "refund": refund,
        }),
    );
}

pub(crate) fn cancel(request_id: u64, receiver_id: &AccountId, cancelled_by: &AccountId) {
    emit_event(
        "cancel",
        serde_json::json!({
            "request_id": request_id,
            "receiver_id": receiver_id,
            "cancelled_by": cancelled_by,
        }),
    );
}
