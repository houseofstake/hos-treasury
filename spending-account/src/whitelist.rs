use near_sdk::json_types::U128;

use crate::*;

#[near]
impl Contract {
    /// Whitelists recipients for native NEAR (`token_id: None`) or NEP-141
    /// tokens. Any invalid entry panics and reverts the whole batch.
    pub fn add_to_whitelist(&mut self, entries: Vec<WhitelistEntry>) {
        self.assert_manager();
        require!(!entries.is_empty(), "No entries provided");
        for entry in entries {
            require!(
                entry.account_id != env::current_account_id(),
                "Cannot whitelist the contract itself"
            );
            let key = (entry.account_id, entry.token_id);
            require!(
                !self.whitelist.contains_key(&key),
                "Account is already whitelisted"
            );
            self.whitelist.insert(key, entry.limit.0);
        }
    }

    /// Removes whitelist entries.
    pub fn remove_from_whitelist(&mut self, keys: Vec<WhitelistEntryKey>) {
        self.assert_manager();
        require!(!keys.is_empty(), "No entries provided");
        for key in keys {
            require!(
                self.whitelist
                    .remove(&(key.account_id, key.token_id))
                    .is_some(),
                "Account is not whitelisted"
            );
        }
    }

    /// Overwrites the remaining limits of whitelist entries.
    pub fn set_limit(&mut self, entries: Vec<WhitelistEntry>) {
        self.assert_manager();
        require!(!entries.is_empty(), "No entries provided");
        for entry in entries {
            let key = (entry.account_id, entry.token_id);
            require!(
                self.whitelist.contains_key(&key),
                "Account is not whitelisted"
            );
            self.whitelist.insert(key, entry.limit.0);
        }
    }

    /// Raises limits by each change's `amount`, creating missing entries.
    pub fn increase_limit(&mut self, changes: Vec<LimitChange>) {
        self.assert_manager();
        require!(!changes.is_empty(), "No entries provided");
        for change in changes {
            require!(
                change.account_id != env::current_account_id(),
                "Cannot whitelist the contract itself"
            );
            let key = (change.account_id, change.token_id);
            let limit = self.whitelist.get(&key).copied().unwrap_or(0);
            let raised = limit.checked_add(change.amount.0).expect("Limit overflow");
            self.whitelist.insert(key, raised);
        }
    }

    /// Lowers limits by each change's `amount`, flooring at zero.
    pub fn decrease_limit(&mut self, changes: Vec<LimitChange>) {
        self.assert_manager();
        require!(!changes.is_empty(), "No entries provided");
        for change in changes {
            let key = (change.account_id, change.token_id);
            let limit = self
                .whitelist
                .get(&key)
                .copied()
                .expect("Account is not whitelisted");
            self.whitelist
                .insert(key, limit.saturating_sub(change.amount.0));
        }
    }

    pub fn get_num_whitelisted(&self) -> u32 {
        self.whitelist.len()
    }

    /// `token_id: None` returns the native NEAR entry, `Some` a token entry.
    pub fn get_whitelist_entry(
        &self,
        account_id: AccountId,
        token_id: Option<AccountId>,
    ) -> Option<WhitelistEntry> {
        self.whitelist
            .get(&(account_id.clone(), token_id.clone()))
            .copied()
            .map(|limit| WhitelistEntry {
                account_id,
                token_id,
                limit: U128(limit),
            })
    }

    /// Returns whitelist entries of both kinds. The order is arbitrary.
    pub fn get_whitelist_entries(
        &self,
        from_index: Option<u32>,
        limit: Option<u32>,
    ) -> Vec<WhitelistEntry> {
        let from_index =
            usize::try_from(from_index.unwrap_or(0)).expect("from_index exceeds usize");
        let limit = usize::try_from(limit.unwrap_or(u32::MAX)).expect("limit exceeds usize");
        self.whitelist
            .iter()
            .skip(from_index)
            .take(limit)
            .map(|((account_id, token_id), limit)| WhitelistEntry {
                account_id: account_id.clone(),
                token_id: token_id.clone(),
                limit: U128(*limit),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils;
    use crate::test_utils::*;
    use near_sdk::testing_env;

    const LIMIT: u128 = 100;

    fn contract_with_alice(limit: u128) -> Contract {
        test_utils::contract_with_alice(Some(token()), U128(limit))
    }

    fn remaining(contract: &Contract) -> u128 {
        let entry = contract
            .get_whitelist_entry(alice(), Some(token()))
            .unwrap();
        entry.limit.0
    }

    #[test]
    fn test_add_to_whitelist() {
        let contract = contract_with_alice(LIMIT);
        assert_eq!(contract.get_num_whitelisted(), 1);
        let entry = contract
            .get_whitelist_entry(alice(), Some(token()))
            .unwrap();
        assert_eq!(entry.account_id, alice());
        assert_eq!(entry.limit, U128(LIMIT));
        assert_eq!(entry.token_id, Some(token()));
        assert_eq!(contract.get_whitelist_entries(None, None).len(), 1);
    }

    #[test]
    #[should_panic(expected = "Account is already whitelisted")]
    fn test_add_duplicate_fails() {
        let mut contract = contract_with_alice(LIMIT);
        contract.add_to_whitelist(entries(alice(), Some(token()), U128(1)));
    }

    #[test]
    #[should_panic(expected = "Cannot whitelist the contract itself")]
    fn test_add_self_fails() {
        testing_env!(context(manager()).build());
        let mut contract = new_contract();
        contract.add_to_whitelist(entries(treasury(), Some(token()), U128(1)));
    }

    #[test]
    #[should_panic(expected = "Only the manager")]
    fn test_add_by_spender_fails() {
        testing_env!(context(manager()).build());
        let mut contract = new_contract();
        testing_env!(context(spender()).build());
        contract.add_to_whitelist(entries(alice(), Some(token()), U128(1)));
    }

    #[test]
    #[should_panic(expected = "Only the manager")]
    fn test_add_by_admin_fails() {
        testing_env!(context(manager()).build());
        let mut contract = new_contract();
        testing_env!(context(admin()).build());
        contract.add_to_whitelist(entries(alice(), Some(token()), U128(1)));
    }

    #[test]
    fn test_remove_from_whitelist() {
        let mut contract = contract_with_alice(LIMIT);
        contract.remove_from_whitelist(keys(alice(), Some(token())));
        assert!(
            contract
                .get_whitelist_entry(alice(), Some(token()))
                .is_none()
        );
        assert_eq!(contract.get_num_whitelisted(), 0);
    }

    #[test]
    #[should_panic(expected = "Account is not whitelisted")]
    fn test_remove_missing_fails() {
        testing_env!(context(manager()).build());
        let mut contract = new_contract();
        contract.remove_from_whitelist(keys(alice(), Some(token())));
    }

    #[test]
    #[should_panic(expected = "Only the manager")]
    fn test_remove_by_other_fails() {
        let mut contract = contract_with_alice(LIMIT);
        testing_env!(context(alice()).build());
        contract.remove_from_whitelist(keys(alice(), Some(token())));
    }

    #[test]
    fn test_set_limit_overrides_remaining() {
        let mut contract = contract_with_alice(LIMIT);
        testing_env!(context(spender()).build());
        contract.transfer_ft(alice(), token(), U128(30));
        testing_env!(context(manager()).build());
        contract.set_limit(entries(alice(), Some(token()), U128(40)));
        assert_eq!(remaining(&contract), 40);
    }

    #[test]
    #[should_panic(expected = "Account is not whitelisted")]
    fn test_set_limit_missing_fails() {
        testing_env!(context(manager()).build());
        let mut contract = new_contract();
        contract.set_limit(entries(alice(), Some(token()), U128(1)));
    }

    #[test]
    fn test_increase_limit() {
        let mut contract = contract_with_alice(LIMIT);
        contract.increase_limit(changes(alice(), Some(token()), U128(50)));
        assert_eq!(remaining(&contract), LIMIT + 50);
    }

    #[test]
    #[should_panic(expected = "Limit overflow")]
    fn test_increase_limit_overflow_fails() {
        let mut contract = contract_with_alice(LIMIT);
        contract.increase_limit(changes(alice(), Some(token()), U128(u128::MAX)));
    }

    #[test]
    fn test_decrease_limit_saturates_at_zero() {
        let mut contract = contract_with_alice(LIMIT);
        contract.decrease_limit(changes(alice(), Some(token()), U128(30)));
        assert_eq!(remaining(&contract), LIMIT - 30);
        // A decrease larger than the current limit zeroes it.
        contract.decrease_limit(changes(alice(), Some(token()), U128(LIMIT * 2)));
        assert_eq!(remaining(&contract), 0);
    }

    #[test]
    fn test_increase_limit_creates_missing_entry() {
        testing_env!(context(manager()).build());
        let mut contract = new_contract();
        contract.increase_limit(changes(alice(), Some(token()), U128(1)));
        assert_eq!(remaining(&contract), 1);
    }

    #[test]
    #[should_panic(expected = "Cannot whitelist the contract itself")]
    fn test_increase_limit_self_fails() {
        testing_env!(context(manager()).build());
        let mut contract = new_contract();
        contract.increase_limit(changes(treasury(), Some(token()), U128(1)));
    }

    #[test]
    #[should_panic(expected = "Account is not whitelisted")]
    fn test_decrease_limit_missing_fails() {
        testing_env!(context(manager()).build());
        let mut contract = new_contract();
        contract.decrease_limit(changes(alice(), Some(token()), U128(1)));
    }

    #[test]
    #[should_panic(expected = "Only the manager")]
    fn test_increase_limit_by_spender_fails() {
        let mut contract = contract_with_alice(LIMIT);
        testing_env!(context(spender()).build());
        contract.increase_limit(changes(alice(), Some(token()), U128(1)));
    }

    #[test]
    fn test_readd_restores_limit() {
        let mut contract = contract_with_alice(LIMIT);
        testing_env!(context(spender()).build());
        contract.transfer_ft(alice(), token(), U128(LIMIT));
        testing_env!(context(manager()).build());
        contract.remove_from_whitelist(keys(alice(), Some(token())));
        contract.add_to_whitelist(entries(alice(), Some(token()), U128(LIMIT)));
        assert_eq!(remaining(&contract), LIMIT);
    }

    #[test]
    fn test_entry_does_not_grant_other_tokens() {
        let contract = contract_with_alice(LIMIT);
        assert!(
            contract
                .get_whitelist_entry(alice(), Some(other_token()))
                .is_none()
        );
        // The token entry grants no native NEAR allowance either.
        assert!(contract.get_whitelist_entry(alice(), None).is_none());
    }

    #[test]
    fn test_add_same_receiver_other_token() {
        let mut contract = contract_with_alice(LIMIT);
        contract.add_to_whitelist(entries(alice(), Some(other_token()), U128(1)));
        contract.add_to_whitelist(entries(alice(), None, yocto(1)));
        assert_eq!(contract.get_num_whitelisted(), 3);
    }

    #[test]
    #[should_panic(expected = "Account is not whitelisted")]
    fn test_token_entry_does_not_grant_near_limit() {
        // `token_id: None` targets the native NEAR entry, which alice lacks.
        let mut contract = contract_with_alice(LIMIT);
        contract.set_limit(entries(alice(), None, U128(1)));
    }

    #[test]
    fn test_batch_add_and_set_limits() {
        let mut contract = contract_with_alice(LIMIT);
        contract.add_to_whitelist(vec![
            WhitelistEntry {
                account_id: alice(),
                token_id: Some(other_token()),
                limit: U128(1),
            },
            WhitelistEntry {
                account_id: alice(),
                token_id: None,
                limit: yocto(1),
            },
        ]);
        assert_eq!(contract.get_num_whitelisted(), 3);
        contract.set_limit(vec![
            WhitelistEntry {
                account_id: alice(),
                token_id: Some(token()),
                limit: U128(40),
            },
            WhitelistEntry {
                account_id: alice(),
                token_id: Some(other_token()),
                limit: U128(2),
            },
        ]);
        assert_eq!(remaining(&contract), 40);
        let entry = contract
            .get_whitelist_entry(alice(), Some(other_token()))
            .unwrap();
        assert_eq!(entry.limit, U128(2));
    }

    #[test]
    fn test_batch_change_limits_and_remove() {
        let mut contract = contract_with_alice(LIMIT);
        contract.add_to_whitelist(entries(alice(), None, yocto(1)));
        contract.increase_limit(vec![
            LimitChange {
                account_id: alice(),
                token_id: Some(token()),
                amount: U128(50),
            },
            LimitChange {
                account_id: alice(),
                token_id: None,
                amount: yocto(2),
            },
        ]);
        assert_eq!(remaining(&contract), LIMIT + 50);
        contract.decrease_limit(vec![
            LimitChange {
                account_id: alice(),
                token_id: Some(token()),
                amount: U128(50),
            },
            LimitChange {
                account_id: alice(),
                token_id: None,
                amount: yocto(3),
            },
        ]);
        assert_eq!(remaining(&contract), LIMIT);
        // The NEAR entry saturated at zero.
        let entry = contract.get_whitelist_entry(alice(), None).unwrap();
        assert_eq!(entry.limit, U128(0));
        contract.remove_from_whitelist(vec![
            WhitelistEntryKey {
                account_id: alice(),
                token_id: Some(token()),
            },
            WhitelistEntryKey {
                account_id: alice(),
                token_id: None,
            },
        ]);
        assert_eq!(contract.get_num_whitelisted(), 0);
    }

    #[test]
    #[should_panic(expected = "Account is already whitelisted")]
    fn test_batch_with_duplicate_entries_fails() {
        testing_env!(context(manager()).build());
        let mut contract = new_contract();
        let entry = || WhitelistEntry {
            account_id: alice(),
            token_id: None,
            limit: yocto(1),
        };
        contract.add_to_whitelist(vec![entry(), entry()]);
    }

    #[test]
    #[should_panic(expected = "No entries provided")]
    fn test_empty_batch_fails() {
        testing_env!(context(manager()).build());
        let mut contract = new_contract();
        contract.add_to_whitelist(Vec::new());
    }
}
