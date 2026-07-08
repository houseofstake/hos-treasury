use crate::*;

#[near]
impl Contract {
    /// Changes the DAO account that controls this timelock.
    pub fn set_dao(&mut self, dao_id: AccountId) {
        self.assert_self();
        self.dao_id = dao_id;
    }

    /// Replaces the guardians list.
    pub fn set_guardians(&mut self, guardians: Vec<AccountId>) {
        self.assert_self();
        self.guardians.clear();
        for guardian in guardians {
            self.guardians.insert(guardian);
        }
    }

    /// Changes the execution delay for requests scheduled after this call.
    pub fn set_delay(&mut self, delay_ns: U64) {
        self.assert_self();
        require!(delay_ns.0 <= MAX_DELAY_NS, "Delay exceeds the maximum");
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
