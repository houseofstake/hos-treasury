//! Shared helpers for unit tests across the contract's modules.

use near_sdk::json_types::U128;
use near_sdk::test_utils::VMContextBuilder;
use near_sdk::testing_env;
use near_sdk::{AccountId, NearToken};

use crate::{Contract, LimitChange, WhitelistEntry, WhitelistEntryKey};

/// A limit/amount of whole NEAR as the yoctoNEAR `U128` the whitelist
/// methods take for `token_id: None`.
pub(crate) fn yocto(near: u128) -> U128 {
    U128(NearToken::from_near(near).as_yoctonear())
}

pub(crate) fn admin() -> AccountId {
    "admin-timelock.near".parse().unwrap()
}

pub(crate) fn manager() -> AccountId {
    "manager-timelock.near".parse().unwrap()
}

pub(crate) fn spender() -> AccountId {
    "spender-timelock.near".parse().unwrap()
}

pub(crate) fn treasury() -> AccountId {
    "treasury.near".parse().unwrap()
}

pub(crate) fn alice() -> AccountId {
    "alice.near".parse().unwrap()
}

pub(crate) fn token() -> AccountId {
    "token.near".parse().unwrap()
}

pub(crate) fn other_token() -> AccountId {
    "other-token.near".parse().unwrap()
}

pub(crate) fn context(predecessor: AccountId) -> VMContextBuilder {
    let mut builder = VMContextBuilder::new();
    builder
        .current_account_id(treasury())
        .predecessor_account_id(predecessor);
    builder
}

pub(crate) fn new_contract() -> Contract {
    Contract::new(admin(), manager(), spender())
}

/// Builds the single-element `entries` argument of `add_to_whitelist` /
/// `set_limit`.
pub(crate) fn entries(
    account_id: AccountId,
    token_id: Option<AccountId>,
    limit: U128,
) -> Vec<WhitelistEntry> {
    vec![WhitelistEntry {
        account_id,
        token_id,
        limit,
    }]
}

/// Builds the single-element `keys` argument of `remove_from_whitelist`.
pub(crate) fn keys(account_id: AccountId, token_id: Option<AccountId>) -> Vec<WhitelistEntryKey> {
    vec![WhitelistEntryKey {
        account_id,
        token_id,
    }]
}

/// Builds the single-element `changes` argument of `increase_limit` /
/// `decrease_limit`.
pub(crate) fn changes(
    account_id: AccountId,
    token_id: Option<AccountId>,
    amount: U128,
) -> Vec<LimitChange> {
    vec![LimitChange {
        account_id,
        token_id,
        amount,
    }]
}

/// A fresh contract with alice whitelisted for `token_id` at `limit`.
/// Leaves the manager as the predecessor in the testing environment.
pub(crate) fn contract_with_alice(token_id: Option<AccountId>, limit: U128) -> Contract {
    testing_env!(context(manager()).build());
    let mut contract = new_contract();
    contract.add_to_whitelist(entries(alice(), token_id, limit));
    contract
}
