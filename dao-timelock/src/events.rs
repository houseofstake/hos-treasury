//! `EVENT_JSON` log emission for all state changes.

use near_sdk::env;

const EVENT_STANDARD: &str = "dao-timelock";
const EVENT_VERSION: &str = "1.0.0";

pub(crate) fn emit_event(event: &str, data: serde_json::Value) {
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
