//! # Spending Account
//!
//! A treasury account that holds a large NEAR balance and pays it out only to
//! whitelisted accounts, each capped by a yearly spending limit.
//!
//! Control is split between two accounts, each expected to be a Sputnik DAO acting
//! through its own `dao-timelock`:
//!
//! - The **spender** can only do one thing: transfer NEAR to an already-whitelisted
//!   account, within that account's remaining yearly allowance.
//! - The **admin** manages everything else: the whitelist, the yearly limits, and
//!   the spender/admin role assignments. The admin cannot transfer funds directly.
//!
//! Yearly limits are enforced over fixed 365-day periods counted from
//! `first_period_start` (set at init, e.g. to a calendar year boundary). Each
//! whitelisted account's spent amount resets at the start of every period.
//!
//! If an outgoing transfer fails (e.g. the receiver account was deleted), the
//! protocol refunds the amount to this contract and a callback rolls back the
//! spent counter, so failed transfers do not consume the allowance.
//!
//! The contract is intentionally NOT upgradable: there is no method to deploy code
//! or migrate state.

use near_sdk::json_types::U64;
use near_sdk::store::IterableMap;
use near_sdk::{
    env, near, require, AccountId, BorshStorageKey, Gas, NearToken, PanicOnDefault, Promise,
    PromiseError,
};

use crate::events::emit_event;

mod admin;
mod events;

/// Length of one spending period: 365 days in nanoseconds.
pub const PERIOD_NS: u64 = 365 * 24 * 60 * 60 * 1_000_000_000;

/// Gas reserved for the transfer-result callback.
const ON_TRANSFER_GAS: Gas = Gas::from_tgas(10);

#[derive(BorshStorageKey)]
#[near(serializers = [borsh])]
enum StorageKey {
    Whitelist,
}

/// Per-recipient spending state. `spent` is only meaningful for `period_index`;
/// when a newer period is observed, it is treated as (and lazily reset to) zero.
#[near(serializers = [borsh])]
pub struct SpendingRecord {
    /// Maximum amount this account can receive within one period.
    yearly_limit: NearToken,
    /// Amount transferred to this account during `period_index`.
    spent: NearToken,
    /// The period `spent` was accumulated in.
    period_index: u64,
}

/// A whitelisted account with its allowance, as returned by view methods.
/// `spent` and `available` are computed for the current period.
#[near(serializers = [json])]
pub struct WhitelistEntry {
    pub account_id: AccountId,
    pub yearly_limit: NearToken,
    pub spent: NearToken,
    pub available: NearToken,
}

impl WhitelistEntry {
    fn new(account_id: AccountId, record: &SpendingRecord, current_period: u64) -> Self {
        let spent = if record.period_index == current_period {
            record.spent
        } else {
            NearToken::from_yoctonear(0)
        };
        Self {
            account_id,
            yearly_limit: record.yearly_limit,
            spent,
            available: record.yearly_limit.saturating_sub(spent),
        }
    }
}

/// The current spending period, as returned by `get_period_info`.
#[near(serializers = [json])]
pub struct PeriodInfo {
    pub period_index: u64,
    /// Block timestamp (ns) at which the current period started.
    pub period_start: U64,
    /// Block timestamp (ns) at which the next period starts.
    pub period_end: U64,
    pub first_period_start: U64,
    pub period_duration_ns: U64,
}

#[near(contract_state)]
#[derive(PanicOnDefault)]
pub struct Contract {
    /// The only account allowed to transfer funds (a DAO's timelock).
    spender_id: AccountId,
    /// The only account allowed to manage the whitelist, limits and roles
    /// (another DAO's timelock).
    admin_id: AccountId,
    /// Start of period 0, in nanoseconds. Periods are `PERIOD_NS` long.
    first_period_start: u64,
    /// Whitelisted recipients and their spending state.
    whitelist: IterableMap<AccountId, SpendingRecord>,
}

#[near]
impl Contract {
    /// Initializes the contract.
    ///
    /// `first_period_start` (ns) anchors the yearly periods, e.g. to a calendar
    /// year boundary; it must not be in the future and defaults to the current
    /// block timestamp.
    #[init]
    pub fn new(
        admin_id: AccountId,
        spender_id: AccountId,
        first_period_start: Option<U64>,
    ) -> Self {
        let first_period_start = first_period_start
            .map(|ts| ts.0)
            .unwrap_or_else(env::block_timestamp);
        require!(
            first_period_start <= env::block_timestamp(),
            "first_period_start must not be in the future"
        );
        Self {
            spender_id,
            admin_id,
            first_period_start,
            whitelist: IterableMap::new(StorageKey::Whitelist),
        }
    }

    /// Transfers `amount` to a whitelisted account. Only callable by the spender.
    ///
    /// Panics if the receiver is not whitelisted or if the transfer would exceed
    /// its remaining allowance for the current period. The allowance is consumed
    /// before the transfer is sent and rolled back by `on_transfer` if the
    /// transfer fails.
    pub fn transfer(&mut self, receiver_id: AccountId, amount: NearToken) -> Promise {
        require!(
            env::predecessor_account_id() == self.spender_id,
            "Only the spender can transfer"
        );
        require!(!amount.is_zero(), "Amount must be positive");
        let period_index = self.current_period_index();
        let record = self
            .whitelist
            .get_mut(&receiver_id)
            .expect("Receiver is not whitelisted");
        if record.period_index != period_index {
            record.spent = NearToken::from_yoctonear(0);
            record.period_index = period_index;
        }
        let new_spent = record
            .spent
            .checked_add(amount)
            .expect("Spent amount overflow");
        require!(
            new_spent <= record.yearly_limit,
            "Transfer exceeds the yearly limit"
        );
        record.spent = new_spent;
        emit_event(
            "transfer",
            serde_json::json!({
                "receiver_id": receiver_id,
                "amount": amount,
                "spent": new_spent,
                "period_index": period_index,
            }),
        );
        Promise::new(receiver_id.clone()).transfer(amount).then(
            Self::ext(env::current_account_id())
                .with_static_gas(ON_TRANSFER_GAS)
                .on_transfer(receiver_id, amount, period_index),
        )
    }

    /// Rolls back the spent counter if the transfer failed. The refunded amount
    /// stays on this contract's balance. The rollback is skipped if the record
    /// was removed or its period rolled over in the meantime (the counter was
    /// already reset).
    #[private]
    pub fn on_transfer(
        &mut self,
        receiver_id: AccountId,
        amount: NearToken,
        period_index: u64,
        #[callback_result] result: Result<(), PromiseError>,
    ) {
        if result.is_ok() {
            return;
        }
        if let Some(record) = self.whitelist.get_mut(&receiver_id) {
            if record.period_index == period_index {
                record.spent = record.spent.saturating_sub(amount);
            }
        }
        emit_event(
            "transfer_failed",
            serde_json::json!({
                "receiver_id": receiver_id,
                "amount": amount,
                "period_index": period_index,
            }),
        );
    }

    pub fn get_spender(&self) -> &AccountId {
        &self.spender_id
    }

    pub fn get_admin(&self) -> &AccountId {
        &self.admin_id
    }

    pub fn is_whitelisted(&self, account_id: AccountId) -> bool {
        self.whitelist.contains_key(&account_id)
    }

    pub fn get_num_whitelisted(&self) -> u32 {
        self.whitelist.len()
    }

    pub fn get_whitelist_entry(&self, account_id: AccountId) -> Option<WhitelistEntry> {
        let period_index = self.current_period_index();
        self.whitelist
            .get(&account_id)
            .map(|record| WhitelistEntry::new(account_id, record, period_index))
    }

    /// Returns whitelisted accounts. The order is arbitrary.
    pub fn get_whitelist(
        &self,
        from_index: Option<u32>,
        limit: Option<u32>,
    ) -> Vec<WhitelistEntry> {
        let period_index = self.current_period_index();
        let from_index = from_index.unwrap_or(0) as usize;
        let limit = limit.unwrap_or(u32::MAX) as usize;
        self.whitelist
            .iter()
            .skip(from_index)
            .take(limit)
            .map(|(account_id, record)| {
                WhitelistEntry::new(account_id.clone(), record, period_index)
            })
            .collect()
    }

    pub fn get_period_info(&self) -> PeriodInfo {
        let period_index = self.current_period_index();
        let period_start = self.first_period_start + period_index * PERIOD_NS;
        PeriodInfo {
            period_index,
            period_start: U64(period_start),
            period_end: U64(period_start + PERIOD_NS),
            first_period_start: U64(self.first_period_start),
            period_duration_ns: U64(PERIOD_NS),
        }
    }
}

impl Contract {
    pub(crate) fn current_period_index(&self) -> u64 {
        env::block_timestamp().saturating_sub(self.first_period_start) / PERIOD_NS
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use near_sdk::test_utils::VMContextBuilder;
    use near_sdk::testing_env;

    const START_TS: u64 = 1_000_000_000_000_000_000;

    fn admin() -> AccountId {
        "admin-timelock.near".parse().unwrap()
    }

    fn spender() -> AccountId {
        "spender-timelock.near".parse().unwrap()
    }

    fn treasury() -> AccountId {
        "treasury.near".parse().unwrap()
    }

    fn alice() -> AccountId {
        "alice.near".parse().unwrap()
    }

    fn context(predecessor: AccountId) -> VMContextBuilder {
        let mut builder = VMContextBuilder::new();
        builder
            .current_account_id(treasury())
            .predecessor_account_id(predecessor)
            .block_timestamp(START_TS);
        builder
    }

    fn new_contract() -> Contract {
        Contract::new(admin(), spender(), None)
    }

    fn contract_with_alice(limit: u128) -> Contract {
        testing_env!(context(admin()).build());
        let mut contract = new_contract();
        contract.add_to_whitelist(alice(), NearToken::from_near(limit));
        contract
    }

    #[test]
    fn test_init_and_views() {
        testing_env!(context(admin()).build());
        let contract = new_contract();
        assert_eq!(contract.get_admin(), &admin());
        assert_eq!(contract.get_spender(), &spender());
        assert_eq!(contract.get_num_whitelisted(), 0);
        assert!(!contract.is_whitelisted(alice()));
        let period = contract.get_period_info();
        assert_eq!(period.period_index, 0);
        assert_eq!(period.first_period_start, U64(START_TS));
        assert_eq!(period.period_start, U64(START_TS));
        assert_eq!(period.period_end, U64(START_TS + PERIOD_NS));
    }

    #[test]
    #[should_panic(expected = "first_period_start must not be in the future")]
    fn test_init_future_period_start_fails() {
        testing_env!(context(admin()).build());
        Contract::new(admin(), spender(), Some(U64(START_TS + 1)));
    }

    #[test]
    fn test_init_past_period_start() {
        testing_env!(context(admin()).build());
        let contract = Contract::new(admin(), spender(), Some(U64(START_TS - PERIOD_NS)));
        assert_eq!(contract.get_period_info().period_index, 1);
    }

    #[test]
    fn test_add_to_whitelist() {
        let contract = contract_with_alice(100);
        assert!(contract.is_whitelisted(alice()));
        assert_eq!(contract.get_num_whitelisted(), 1);
        let entry = contract.get_whitelist_entry(alice()).unwrap();
        assert_eq!(entry.yearly_limit, NearToken::from_near(100));
        assert_eq!(entry.spent, NearToken::from_yoctonear(0));
        assert_eq!(entry.available, NearToken::from_near(100));
        assert_eq!(contract.get_whitelist(None, None).len(), 1);
    }

    #[test]
    #[should_panic(expected = "Account is already whitelisted")]
    fn test_add_duplicate_fails() {
        let mut contract = contract_with_alice(100);
        contract.add_to_whitelist(alice(), NearToken::from_near(1));
    }

    #[test]
    #[should_panic(expected = "Cannot whitelist the contract itself")]
    fn test_add_self_fails() {
        testing_env!(context(admin()).build());
        let mut contract = new_contract();
        contract.add_to_whitelist(treasury(), NearToken::from_near(1));
    }

    #[test]
    #[should_panic(expected = "Only the admin")]
    fn test_add_by_spender_fails() {
        testing_env!(context(admin()).build());
        let mut contract = new_contract();
        testing_env!(context(spender()).build());
        contract.add_to_whitelist(alice(), NearToken::from_near(1));
    }

    #[test]
    fn test_remove_from_whitelist() {
        let mut contract = contract_with_alice(100);
        contract.remove_from_whitelist(alice());
        assert!(!contract.is_whitelisted(alice()));
        assert_eq!(contract.get_num_whitelisted(), 0);
    }

    #[test]
    #[should_panic(expected = "Account is not whitelisted")]
    fn test_remove_missing_fails() {
        testing_env!(context(admin()).build());
        let mut contract = new_contract();
        contract.remove_from_whitelist(alice());
    }

    #[test]
    #[should_panic(expected = "Only the admin")]
    fn test_remove_by_other_fails() {
        let mut contract = contract_with_alice(100);
        testing_env!(context(alice()).build());
        contract.remove_from_whitelist(alice());
    }

    #[test]
    fn test_set_yearly_limit_keeps_spent() {
        let mut contract = contract_with_alice(100);
        testing_env!(context(spender()).build());
        contract.transfer(alice(), NearToken::from_near(30));
        testing_env!(context(admin()).build());
        contract.set_yearly_limit(alice(), NearToken::from_near(40));
        let entry = contract.get_whitelist_entry(alice()).unwrap();
        assert_eq!(entry.yearly_limit, NearToken::from_near(40));
        assert_eq!(entry.spent, NearToken::from_near(30));
        assert_eq!(entry.available, NearToken::from_near(10));
        // Lowering the limit below the spent amount leaves zero available.
        contract.set_yearly_limit(alice(), NearToken::from_near(20));
        let entry = contract.get_whitelist_entry(alice()).unwrap();
        assert_eq!(entry.available, NearToken::from_yoctonear(0));
    }

    #[test]
    #[should_panic(expected = "Account is not whitelisted")]
    fn test_set_yearly_limit_missing_fails() {
        testing_env!(context(admin()).build());
        let mut contract = new_contract();
        contract.set_yearly_limit(alice(), NearToken::from_near(1));
    }

    #[test]
    fn test_set_spender_and_admin() {
        testing_env!(context(admin()).build());
        let mut contract = new_contract();
        let new_spender: AccountId = "spender2.near".parse().unwrap();
        let new_admin: AccountId = "admin2.near".parse().unwrap();
        contract.set_spender(new_spender.clone());
        assert_eq!(contract.get_spender(), &new_spender);
        contract.set_admin(new_admin.clone());
        assert_eq!(contract.get_admin(), &new_admin);
        // The old admin lost control.
        testing_env!(context(admin()).build());
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            contract.set_spender(spender());
        }));
        assert!(result.is_err());
    }

    #[test]
    #[should_panic(expected = "Only the admin")]
    fn test_set_spender_by_spender_fails() {
        testing_env!(context(admin()).build());
        let mut contract = new_contract();
        testing_env!(context(spender()).build());
        contract.set_spender(spender());
    }

    #[test]
    fn test_transfer_within_limit() {
        let mut contract = contract_with_alice(100);
        testing_env!(context(spender()).build());
        contract.transfer(alice(), NearToken::from_near(60));
        contract.transfer(alice(), NearToken::from_near(40));
        let entry = contract.get_whitelist_entry(alice()).unwrap();
        assert_eq!(entry.spent, NearToken::from_near(100));
        assert_eq!(entry.available, NearToken::from_yoctonear(0));
    }

    #[test]
    #[should_panic(expected = "Transfer exceeds the yearly limit")]
    fn test_transfer_over_limit_fails() {
        let mut contract = contract_with_alice(100);
        testing_env!(context(spender()).build());
        contract.transfer(alice(), NearToken::from_near(101));
    }

    #[test]
    #[should_panic(expected = "Transfer exceeds the yearly limit")]
    fn test_transfer_cumulative_over_limit_fails() {
        let mut contract = contract_with_alice(100);
        testing_env!(context(spender()).build());
        contract.transfer(alice(), NearToken::from_near(60));
        contract.transfer(alice(), NearToken::from_near(41));
    }

    #[test]
    #[should_panic(expected = "Only the spender can transfer")]
    fn test_transfer_by_admin_fails() {
        let mut contract = contract_with_alice(100);
        testing_env!(context(admin()).build());
        contract.transfer(alice(), NearToken::from_near(1));
    }

    #[test]
    #[should_panic(expected = "Receiver is not whitelisted")]
    fn test_transfer_to_non_whitelisted_fails() {
        testing_env!(context(admin()).build());
        let mut contract = new_contract();
        testing_env!(context(spender()).build());
        contract.transfer(alice(), NearToken::from_near(1));
    }

    #[test]
    #[should_panic(expected = "Amount must be positive")]
    fn test_transfer_zero_fails() {
        let mut contract = contract_with_alice(100);
        testing_env!(context(spender()).build());
        contract.transfer(alice(), NearToken::from_yoctonear(0));
    }

    #[test]
    fn test_limit_resets_next_period() {
        let mut contract = contract_with_alice(100);
        testing_env!(context(spender()).build());
        contract.transfer(alice(), NearToken::from_near(100));
        // A year later the full limit is available again.
        testing_env!(context(spender())
            .block_timestamp(START_TS + PERIOD_NS)
            .build());
        assert_eq!(contract.get_period_info().period_index, 1);
        let entry = contract.get_whitelist_entry(alice()).unwrap();
        assert_eq!(entry.spent, NearToken::from_yoctonear(0));
        assert_eq!(entry.available, NearToken::from_near(100));
        contract.transfer(alice(), NearToken::from_near(100));
        let entry = contract.get_whitelist_entry(alice()).unwrap();
        assert_eq!(entry.spent, NearToken::from_near(100));
    }

    #[test]
    #[should_panic(expected = "Transfer exceeds the yearly limit")]
    fn test_limit_not_reset_within_period() {
        let mut contract = contract_with_alice(100);
        testing_env!(context(spender()).build());
        contract.transfer(alice(), NearToken::from_near(100));
        testing_env!(context(spender())
            .block_timestamp(START_TS + PERIOD_NS - 1)
            .build());
        contract.transfer(alice(), NearToken::from_near(1));
    }

    #[test]
    fn test_failed_transfer_rolls_back_spent() {
        let mut contract = contract_with_alice(100);
        testing_env!(context(spender()).build());
        contract.transfer(alice(), NearToken::from_near(60));
        testing_env!(context(treasury()).build());
        contract.on_transfer(
            alice(),
            NearToken::from_near(60),
            0,
            Err(PromiseError::Failed),
        );
        let entry = contract.get_whitelist_entry(alice()).unwrap();
        assert_eq!(entry.spent, NearToken::from_yoctonear(0));
        assert_eq!(entry.available, NearToken::from_near(100));
    }

    #[test]
    fn test_successful_transfer_keeps_spent() {
        let mut contract = contract_with_alice(100);
        testing_env!(context(spender()).build());
        contract.transfer(alice(), NearToken::from_near(60));
        testing_env!(context(treasury()).build());
        contract.on_transfer(alice(), NearToken::from_near(60), 0, Ok(()));
        let entry = contract.get_whitelist_entry(alice()).unwrap();
        assert_eq!(entry.spent, NearToken::from_near(60));
    }

    #[test]
    fn test_rollback_skipped_after_period_change() {
        let mut contract = contract_with_alice(100);
        testing_env!(context(spender()).build());
        contract.transfer(alice(), NearToken::from_near(60));
        // The period rolls over and the spender transfers again before the
        // failed transfer's callback lands: the new period's spent must not
        // be reduced by the old period's rollback.
        testing_env!(context(spender())
            .block_timestamp(START_TS + PERIOD_NS)
            .build());
        contract.transfer(alice(), NearToken::from_near(50));
        testing_env!(context(treasury())
            .block_timestamp(START_TS + PERIOD_NS)
            .build());
        contract.on_transfer(
            alice(),
            NearToken::from_near(60),
            0,
            Err(PromiseError::Failed),
        );
        let entry = contract.get_whitelist_entry(alice()).unwrap();
        assert_eq!(entry.spent, NearToken::from_near(50));
    }

    #[test]
    fn test_rollback_after_removal_does_not_panic() {
        let mut contract = contract_with_alice(100);
        testing_env!(context(spender()).build());
        contract.transfer(alice(), NearToken::from_near(60));
        testing_env!(context(admin()).build());
        contract.remove_from_whitelist(alice());
        testing_env!(context(treasury()).build());
        contract.on_transfer(
            alice(),
            NearToken::from_near(60),
            0,
            Err(PromiseError::Failed),
        );
        assert!(!contract.is_whitelisted(alice()));
    }
}
