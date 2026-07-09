//! # Spending Account
//!
//! A treasury that pays out NEAR and NEP-141 tokens (see [`transfer`]) only to
//! whitelisted recipients, each capped by a non-expiring spending limit. The
//! **spender** can only transfer within an entry's remaining allowance; the
//! **admin** manages the whitelist, limits and roles but cannot transfer.
//! Both are expected to be Sputnik DAOs acting through their own `dao-timelock`.
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

/// A whitelist entry with its remaining limit, as returned by view methods.
/// `token_id: None` is native NEAR (`limit` in yoctoNEAR); `Some` is a NEP-141
/// token (`limit` in its smallest unit).
#[near(serializers = [json])]
pub struct WhitelistEntry {
    pub account_id: AccountId,
    pub token_id: Option<AccountId>,
    pub limit: U128,
}

#[near(contract_state)]
#[derive(PanicOnDefault)]
pub struct Contract {
    /// The only account allowed to transfer funds.
    spender_id: AccountId,
    /// The only account allowed to manage the whitelist, limits and roles.
    admin_id: AccountId,
    /// Whitelisted (recipient, token) pairs and their remaining limits, in the
    /// token's smallest unit (yoctoNEAR for native NEAR entries).
    whitelist: IterableMap<WhitelistKey, u128>,
}

#[near]
impl Contract {
    /// Initializes the contract.
    #[init]
    pub fn new(admin_id: AccountId, spender_id: AccountId) -> Self {
        Self {
            spender_id,
            admin_id,
            whitelist: IterableMap::new(StorageKey::Whitelist),
        }
    }

    pub fn get_spender(&self) -> &AccountId {
        &self.spender_id
    }

    pub fn get_admin(&self) -> &AccountId {
        &self.admin_id
    }
}

#[cfg(test)]
mod tests {
    use crate::test_utils::*;
    use near_sdk::testing_env;

    #[test]
    fn test_init_and_views() {
        testing_env!(context(admin()).build());
        let contract = new_contract();
        assert_eq!(contract.get_admin(), &admin());
        assert_eq!(contract.get_spender(), &spender());
        assert_eq!(contract.get_num_whitelisted(), 0);
        assert!(contract.get_whitelist_entry(alice(), None).is_none());
    }
}
