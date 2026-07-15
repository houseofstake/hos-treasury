use crate::*;

#[near]
impl Contract {
    /// Changes the DAO account that controls this timelock.
    pub fn set_dao(&mut self, dao_id: AccountId) {
        self.assert_admin();
        self.dao_id = dao_id;
    }

    /// Transfers the admin role to another account.
    pub fn set_admin(&mut self, admin_id: AccountId) {
        self.assert_admin();
        self.admin_id = admin_id;
    }

    /// Replaces the guardians list.
    pub fn set_guardians(&mut self, guardians: Vec<AccountId>) {
        self.assert_admin();
        self.guardians.clear();
        for guardian in guardians {
            self.guardians.insert(guardian);
        }
    }

    /// Changes the execution delay for requests scheduled after this call.
    pub fn set_delay(&mut self, delay_ns: U64) {
        self.assert_admin();
        require!(delay_ns.0 <= MAX_DELAY_NS, "Delay exceeds the maximum");
        self.delay_ns = delay_ns.0;
    }
}

impl Contract {
    pub(crate) fn assert_admin(&self) {
        require!(
            env::predecessor_account_id() == self.admin_id,
            "Only the admin can call this method"
        );
    }
}
