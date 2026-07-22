//! # Spending Account
//!
//! A treasury that pays out NEAR and NEP-141 tokens (see [`transfer`]) only to
//! whitelisted recipients, each capped by a non-expiring spending limit. The
//! **spender** can only transfer within an entry's remaining allowance; the
//! **manager** manages the whitelist and limits; the **admin** only assigns
//! the roles. All three are expected to be Sputnik DAOs acting through their
//! own `dao-timelock`.
//! Failed transfers are rolled back by a callback and do not consume the
//! allowance. Intentionally NOT upgradable: no method deploys code or migrates
//! state.

use near_sdk::json_types::U128;
use near_sdk::store::IterableMap;
use near_sdk::{
    AccountId, BorshStorageKey, Gas, NearToken, PanicOnDefault, Promise, PromiseError, env, near,
    require,
};

mod admin;
mod events;
#[cfg(test)]
mod test_utils;
pub mod transfer;
mod whitelist;

/// Key of a whitelist entry: (recipient, token contract), where `None` as the
/// token means native NEAR.
pub type WhitelistKey = (AccountId, Option<AccountId>);

#[derive(BorshStorageKey)]
#[near(serializers = [borsh])]
enum StorageKey {
    Whitelist,
}

/// A whitelist entry with its (remaining) limit, as returned by view methods
/// and taken by `add_to_whitelist` / `set_limit`. `token_id: None` is native
/// NEAR (`limit` in yoctoNEAR); `Some` is a NEP-141 token (`limit` in its
/// smallest unit).
#[near(serializers = [json])]
pub struct WhitelistEntry {
    pub account_id: AccountId,
    pub token_id: Option<AccountId>,
    pub limit: U128,
}

/// Identifies a whitelist entry in `remove_from_whitelist` arguments.
#[near(serializers = [json])]
pub struct WhitelistEntryKey {
    pub account_id: AccountId,
    pub token_id: Option<AccountId>,
}

/// A limit adjustment taken by `increase_limit` / `decrease_limit`, with
/// `amount` in the token's smallest unit.
#[near(serializers = [json])]
pub struct LimitChange {
    pub account_id: AccountId,
    pub token_id: Option<AccountId>,
    pub amount: U128,
}

#[near(contract_state)]
#[derive(PanicOnDefault)]
pub struct Contract {
    /// The only account allowed to transfer funds.
    spender_id: AccountId,
    /// The only account allowed to manage the whitelist and limits.
    manager_id: AccountId,
    /// The only account allowed to assign the roles.
    admin_id: AccountId,
    /// Whitelisted (recipient, token) pairs and their remaining limits.
    whitelist: IterableMap<WhitelistKey, u128>,
}

#[near]
impl Contract {
    /// Initializes the contract.
    #[init]
    pub fn new(admin_id: AccountId, manager_id: AccountId, spender_id: AccountId) -> Self {
        Self {
            spender_id,
            manager_id,
            admin_id,
            whitelist: IterableMap::new(StorageKey::Whitelist),
        }
    }

    pub fn get_spender(&self) -> &AccountId {
        &self.spender_id
    }

    pub fn get_manager(&self) -> &AccountId {
        &self.manager_id
    }

    pub fn get_admin(&self) -> &AccountId {
        &self.admin_id
    }
}
