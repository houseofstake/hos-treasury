//! Self-only configuration methods: callable only by the timelock account itself, so
//! every config change must go through a scheduled request and wait out the delay.

use crate::events::emit_event;
use crate::*;

#[near]
impl Contract {
    /// Changes the DAO account that controls this timelock.
    pub fn set_dao(&mut self, dao_id: AccountId) {
        self.assert_self();
        emit_event(
            "set_dao",
            serde_json::json!({ "old_dao_id": self.dao_id, "new_dao_id": dao_id }),
        );
        self.dao_id = dao_id;
    }

    /// Replaces the guardians list.
    pub fn set_guardians(&mut self, guardians: Vec<AccountId>) {
        self.assert_self();
        emit_event("set_guardians", serde_json::json!({ "guardians": guardians }));
        self.guardians.clear();
        for guardian in guardians {
            self.guardians.insert(guardian);
        }
    }

    /// Changes the execution delay for requests scheduled after this call.
    pub fn set_delay(&mut self, delay_ns: U64) {
        self.assert_self();
        require!(delay_ns.0 <= MAX_DELAY_NS, "Delay exceeds the maximum");
        emit_event(
            "set_delay",
            serde_json::json!({ "old_delay_ns": U64(self.delay_ns), "new_delay_ns": delay_ns }),
        );
        self.delay_ns = delay_ns.0;
    }
}

impl Contract {
    pub(crate) fn assert_self(&self) {
        require!(
            env::predecessor_account_id() == env::current_account_id(),
            "Only callable by the timelock itself via a scheduled request"
        );
    }
}
