//! Admin-only management methods.
//!
//! These are only callable by the admin account (expected to be a DAO's timelock),
//! which manages the whitelist, the yearly limits and the role assignments. The
//! admin cannot transfer funds; only the spender can, and only to accounts
//! whitelisted here.

use crate::events::emit_event;
use crate::*;

#[near]
impl Contract {
    /// Adds an account to the whitelist with the given yearly limit.
    ///
    /// Removing and re-adding an account within the same period resets its spent
    /// amount, so its full limit becomes available again.
    pub fn add_to_whitelist(&mut self, account_id: AccountId, yearly_limit: NearToken) {
        self.assert_admin();
        require!(
            account_id != env::current_account_id(),
            "Cannot whitelist the contract itself"
        );
        require!(
            !self.whitelist.contains_key(&account_id),
            "Account is already whitelisted"
        );
        emit_event(
            "add_to_whitelist",
            serde_json::json!({ "account_id": account_id, "yearly_limit": yearly_limit }),
        );
        self.whitelist.insert(
            account_id,
            SpendingRecord {
                yearly_limit,
                spent: NearToken::from_yoctonear(0),
                period_index: self.current_period_index(),
            },
        );
    }

    /// Removes an account from the whitelist, discarding its spending history.
    pub fn remove_from_whitelist(&mut self, account_id: AccountId) {
        self.assert_admin();
        require!(
            self.whitelist.remove(&account_id).is_some(),
            "Account is not whitelisted"
        );
        emit_event(
            "remove_from_whitelist",
            serde_json::json!({ "account_id": account_id }),
        );
    }

    /// Changes the yearly limit of a whitelisted account. The amount already
    /// spent this period is kept: lowering the limit below it just leaves zero
    /// available until the next period.
    pub fn set_yearly_limit(&mut self, account_id: AccountId, yearly_limit: NearToken) {
        self.assert_admin();
        let record = self
            .whitelist
            .get_mut(&account_id)
            .expect("Account is not whitelisted");
        emit_event(
            "set_yearly_limit",
            serde_json::json!({
                "account_id": account_id,
                "old_yearly_limit": record.yearly_limit,
                "new_yearly_limit": yearly_limit,
            }),
        );
        record.yearly_limit = yearly_limit;
    }

    /// Changes the account allowed to transfer funds.
    pub fn set_spender(&mut self, spender_id: AccountId) {
        self.assert_admin();
        emit_event(
            "set_spender",
            serde_json::json!({ "old_spender_id": self.spender_id, "new_spender_id": spender_id }),
        );
        self.spender_id = spender_id;
    }

    /// Transfers the admin role to another account.
    pub fn set_admin(&mut self, admin_id: AccountId) {
        self.assert_admin();
        emit_event(
            "set_admin",
            serde_json::json!({ "old_admin_id": self.admin_id, "new_admin_id": admin_id }),
        );
        self.admin_id = admin_id;
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
