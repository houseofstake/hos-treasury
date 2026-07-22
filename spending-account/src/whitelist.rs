use near_sdk::json_types::U128;

use crate::*;

#[near]
impl Contract {
    /// Whitelists a recipient for native NEAR(`token_id`: None) or a NEP-141 token
    pub fn add_to_whitelist(
        &mut self,
        account_id: AccountId,
        token_id: Option<AccountId>,
        limit: U128,
    ) {
        self.assert_manager();
        require!(
            account_id != env::current_account_id(),
            "Cannot whitelist the contract itself"
        );
        let key = (account_id, token_id);
        require!(
            !self.whitelist.contains_key(&key),
            "Account is already whitelisted"
        );
        self.whitelist.insert(key, limit.0);
    }

    /// Removes a whitelist entry.
    pub fn remove_from_whitelist(&mut self, account_id: AccountId, token_id: Option<AccountId>) {
        self.assert_manager();
        require!(
            self.whitelist.remove(&(account_id, token_id)).is_some(),
            "Account is not whitelisted"
        );
    }

    /// Changes the remaining limit of a whitelist entry.
    pub fn set_limit(&mut self, account_id: AccountId, token_id: Option<AccountId>, limit: U128) {
        self.assert_manager();
        let key = (account_id, token_id);
        require!(
            self.whitelist.contains_key(&key),
            "Account is not whitelisted"
        );
        self.whitelist.insert(key, limit.0);
    }

    /// Raises the limit of a whitelist entry by `amount`.
    pub fn increase_limit(
        &mut self,
        account_id: AccountId,
        token_id: Option<AccountId>,
        amount: U128,
    ) {
        self.assert_manager();
        require!(
            account_id != env::current_account_id(),
            "Cannot whitelist the contract itself"
        );
        let key = (account_id, token_id);
        let limit = self.whitelist.get(&key).copied().unwrap_or(0);
        self.whitelist
            .insert(key, limit.checked_add(amount.0).expect("Limit overflow"));
    }

    /// Lowers the limit of a whitelist entry by `amount`.
    pub fn decrease_limit(
        &mut self,
        account_id: AccountId,
        token_id: Option<AccountId>,
        amount: U128,
    ) {
        self.assert_manager();
        let key = (account_id, token_id);
        let limit = self
            .whitelist
            .get(&key)
            .copied()
            .expect("Account is not whitelisted");
        self.whitelist.insert(key, limit.saturating_sub(amount.0));
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
        contract.add_to_whitelist(alice(), Some(token()), U128(1));
    }

    #[test]
    #[should_panic(expected = "Cannot whitelist the contract itself")]
    fn test_add_self_fails() {
        testing_env!(context(manager()).build());
        let mut contract = new_contract();
        contract.add_to_whitelist(treasury(), Some(token()), U128(1));
    }

    #[test]
    #[should_panic(expected = "Only the manager")]
    fn test_add_by_spender_fails() {
        testing_env!(context(manager()).build());
        let mut contract = new_contract();
        testing_env!(context(spender()).build());
        contract.add_to_whitelist(alice(), Some(token()), U128(1));
    }

    #[test]
    #[should_panic(expected = "Only the manager")]
    fn test_add_by_admin_fails() {
        testing_env!(context(manager()).build());
        let mut contract = new_contract();
        testing_env!(context(admin()).build());
        contract.add_to_whitelist(alice(), Some(token()), U128(1));
    }

    #[test]
    fn test_remove_from_whitelist() {
        let mut contract = contract_with_alice(LIMIT);
        contract.remove_from_whitelist(alice(), Some(token()));
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
        contract.remove_from_whitelist(alice(), Some(token()));
    }

    #[test]
    #[should_panic(expected = "Only the manager")]
    fn test_remove_by_other_fails() {
        let mut contract = contract_with_alice(LIMIT);
        testing_env!(context(alice()).build());
        contract.remove_from_whitelist(alice(), Some(token()));
    }

    #[test]
    fn test_set_limit_overrides_remaining() {
        let mut contract = contract_with_alice(LIMIT);
        testing_env!(context(spender()).build());
        contract.transfer_ft(alice(), token(), U128(30));
        testing_env!(context(manager()).build());
        contract.set_limit(alice(), Some(token()), U128(40));
        assert_eq!(remaining(&contract), 40);
    }

    #[test]
    #[should_panic(expected = "Account is not whitelisted")]
    fn test_set_limit_missing_fails() {
        testing_env!(context(manager()).build());
        let mut contract = new_contract();
        contract.set_limit(alice(), Some(token()), U128(1));
    }

    #[test]
    fn test_increase_limit() {
        let mut contract = contract_with_alice(LIMIT);
        contract.increase_limit(alice(), Some(token()), U128(50));
        assert_eq!(remaining(&contract), LIMIT + 50);
    }

    #[test]
    #[should_panic(expected = "Limit overflow")]
    fn test_increase_limit_overflow_fails() {
        let mut contract = contract_with_alice(LIMIT);
        contract.increase_limit(alice(), Some(token()), U128(u128::MAX));
    }

    #[test]
    fn test_decrease_limit_saturates_at_zero() {
        let mut contract = contract_with_alice(LIMIT);
        contract.decrease_limit(alice(), Some(token()), U128(30));
        assert_eq!(remaining(&contract), LIMIT - 30);
        // A decrease larger than the current limit zeroes it.
        contract.decrease_limit(alice(), Some(token()), U128(LIMIT * 2));
        assert_eq!(remaining(&contract), 0);
    }

    #[test]
    fn test_increase_limit_creates_missing_entry() {
        testing_env!(context(manager()).build());
        let mut contract = new_contract();
        contract.increase_limit(alice(), Some(token()), U128(1));
        assert_eq!(remaining(&contract), 1);
    }

    #[test]
    #[should_panic(expected = "Cannot whitelist the contract itself")]
    fn test_increase_limit_self_fails() {
        testing_env!(context(manager()).build());
        let mut contract = new_contract();
        contract.increase_limit(treasury(), Some(token()), U128(1));
    }

    #[test]
    #[should_panic(expected = "Account is not whitelisted")]
    fn test_decrease_limit_missing_fails() {
        testing_env!(context(manager()).build());
        let mut contract = new_contract();
        contract.decrease_limit(alice(), Some(token()), U128(1));
    }

    #[test]
    #[should_panic(expected = "Only the manager")]
    fn test_increase_limit_by_spender_fails() {
        let mut contract = contract_with_alice(LIMIT);
        testing_env!(context(spender()).build());
        contract.increase_limit(alice(), Some(token()), U128(1));
    }

    #[test]
    fn test_readd_restores_limit() {
        let mut contract = contract_with_alice(LIMIT);
        testing_env!(context(spender()).build());
        contract.transfer_ft(alice(), token(), U128(LIMIT));
        testing_env!(context(manager()).build());
        contract.remove_from_whitelist(alice(), Some(token()));
        contract.add_to_whitelist(alice(), Some(token()), U128(LIMIT));
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
        contract.add_to_whitelist(alice(), Some(other_token()), U128(1));
        contract.add_to_whitelist(alice(), None, yocto(1));
        assert_eq!(contract.get_num_whitelisted(), 3);
    }

    #[test]
    #[should_panic(expected = "Account is not whitelisted")]
    fn test_token_entry_does_not_grant_near_limit() {
        // `token_id: None` targets the native NEAR entry, which alice lacks.
        let mut contract = contract_with_alice(LIMIT);
        contract.set_limit(alice(), None, U128(1));
    }
}
