use near_sdk::GasWeight;
use near_sdk::json_types::U128;

use crate::*;

const ON_TRANSFER_GAS: Gas = Gas::from_tgas(10);
const FT_TRANSFER_GAS: Gas = Gas::from_tgas(10);
const ONE_YOCTO: NearToken = NearToken::from_yoctonear(1);

#[near]
impl Contract {
    /// Transfers near `amount` to a whitelisted account.
    pub fn transfer(&mut self, receiver_id: AccountId, amount: NearToken) -> Promise {
        require!(
            env::predecessor_account_id() == self.spender_id,
            "Only the spender can transfer"
        );
        require!(!amount.is_zero(), "Amount must be positive");
        let key = (receiver_id.clone(), None);
        let limit = self
            .whitelist
            .get(&key)
            .copied()
            .expect("Receiver is not whitelisted");
        require!(amount.as_yoctonear() <= limit, "Transfer exceeds the limit");
        let remaining = limit.saturating_sub(amount.as_yoctonear());
        self.whitelist.insert(key, remaining);
        events::transfer(&receiver_id, amount, NearToken::from_yoctonear(remaining));
        Promise::new(receiver_id.clone()).transfer(amount).then(
            Self::ext(env::current_account_id())
                .with_static_gas(ON_TRANSFER_GAS)
                .on_transfer(receiver_id, amount),
        )
    }

    /// Restores the limit if the transfer failed.
    #[private]
    pub fn on_transfer(
        &mut self,
        receiver_id: AccountId,
        amount: NearToken,
        #[callback_result] result: Result<(), PromiseError>,
    ) {
        if result.is_ok() {
            return;
        }
        let key = (receiver_id.clone(), None);
        if let Some(limit) = self.whitelist.get(&key).copied() {
            self.whitelist
                .insert(key, limit.saturating_add(amount.as_yoctonear()));
        }
        events::transfer_failed(&receiver_id, amount);
    }

    /// Transfers `amount` of `token_id` to a whitelisted receiver.
    pub fn transfer_ft(
        &mut self,
        receiver_id: AccountId,
        token_id: AccountId,
        amount: U128,
    ) -> Promise {
        require!(
            env::predecessor_account_id() == self.spender_id,
            "Only the spender can transfer"
        );
        require!(amount.0 > 0, "Amount must be positive");
        let key = (receiver_id.clone(), Some(token_id.clone()));
        let limit = self
            .whitelist
            .get(&key)
            .copied()
            .expect("Receiver is not whitelisted for this token");
        require!(amount.0 <= limit, "Transfer exceeds the limit");
        let remaining = limit.saturating_sub(amount.0);
        self.whitelist.insert(key, remaining);
        events::ft_transfer(&receiver_id, &token_id, amount, U128(remaining));
        Promise::new(token_id.clone())
            .function_call_weight(
                "ft_transfer".to_string(),
                serde_json::json!({
                    "receiver_id": receiver_id,
                    "amount": amount,
                })
                .to_string()
                .into_bytes(),
                ONE_YOCTO,
                FT_TRANSFER_GAS,
                GasWeight(1),
            )
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(ON_TRANSFER_GAS)
                    .on_transfer_ft(receiver_id, token_id, amount),
            )
    }

    /// Restores the limit if `ft_transfer` failed.
    #[private]
    pub fn on_transfer_ft(
        &mut self,
        receiver_id: AccountId,
        token_id: AccountId,
        amount: U128,
        #[callback_result] result: Result<(), PromiseError>,
    ) {
        if result.is_ok() {
            return;
        }
        let key = (receiver_id.clone(), Some(token_id.clone()));
        if let Some(limit) = self.whitelist.get(&key).copied() {
            self.whitelist.insert(key, limit.saturating_add(amount.0));
        }
        events::ft_transfer_failed(&receiver_id, &token_id, amount);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils;
    use crate::test_utils::*;
    use near_sdk::testing_env;

    const FT_LIMIT: u128 = 100;

    fn near_contract_with_alice(limit: u128) -> Contract {
        test_utils::contract_with_alice(None, yocto(limit))
    }

    fn near_remaining(contract: &Contract) -> U128 {
        contract.get_whitelist_entry(alice(), None).unwrap().limit
    }

    fn ft_contract_with_alice(limit: u128) -> Contract {
        test_utils::contract_with_alice(Some(token()), U128(limit))
    }

    fn ft_remaining(contract: &Contract) -> u128 {
        let entry = contract
            .get_whitelist_entry(alice(), Some(token()))
            .unwrap();
        entry.limit.0
    }

    // Native NEAR transfers (`token_id: None` whitelist entries).

    #[test]
    fn test_near_transfer_within_limit() {
        let mut contract = near_contract_with_alice(100);
        testing_env!(context(spender()).build());
        contract.transfer(alice(), NearToken::from_near(60));
        contract.transfer(alice(), NearToken::from_near(40));
        assert_eq!(near_remaining(&contract), U128(0));
    }

    #[test]
    #[should_panic(expected = "Transfer exceeds the limit")]
    fn test_near_transfer_over_limit_fails() {
        let mut contract = near_contract_with_alice(100);
        testing_env!(context(spender()).build());
        contract.transfer(alice(), NearToken::from_near(101));
    }

    #[test]
    #[should_panic(expected = "Transfer exceeds the limit")]
    fn test_near_transfer_cumulative_over_limit_fails() {
        let mut contract = near_contract_with_alice(100);
        testing_env!(context(spender()).build());
        contract.transfer(alice(), NearToken::from_near(60));
        contract.transfer(alice(), NearToken::from_near(41));
    }

    #[test]
    #[should_panic(expected = "Only the spender can transfer")]
    fn test_near_transfer_by_admin_fails() {
        let mut contract = near_contract_with_alice(100);
        testing_env!(context(admin()).build());
        contract.transfer(alice(), NearToken::from_near(1));
    }

    #[test]
    #[should_panic(expected = "Only the spender can transfer")]
    fn test_near_transfer_by_manager_fails() {
        let mut contract = near_contract_with_alice(100);
        testing_env!(context(manager()).build());
        contract.transfer(alice(), NearToken::from_near(1));
    }

    #[test]
    #[should_panic(expected = "Receiver is not whitelisted")]
    fn test_near_transfer_to_non_whitelisted_fails() {
        testing_env!(context(manager()).build());
        let mut contract = new_contract();
        testing_env!(context(spender()).build());
        contract.transfer(alice(), NearToken::from_near(1));
    }

    #[test]
    #[should_panic(expected = "Amount must be positive")]
    fn test_near_transfer_zero_fails() {
        let mut contract = near_contract_with_alice(100);
        testing_env!(context(spender()).build());
        contract.transfer(alice(), NearToken::from_yoctonear(0));
    }

    #[test]
    fn test_near_raised_limit_extends_allowance() {
        let mut contract = near_contract_with_alice(100);
        testing_env!(context(spender()).build());
        contract.transfer(alice(), NearToken::from_near(100));
        // The allowance is used up; the manager must raise it to allow more.
        testing_env!(context(manager()).build());
        contract.increase_limit(changes(alice(), None, yocto(50)));
        testing_env!(context(spender()).build());
        contract.transfer(alice(), NearToken::from_near(50));
        assert_eq!(near_remaining(&contract), U128(0));
    }

    #[test]
    fn test_near_failed_transfer_restores_limit() {
        let mut contract = near_contract_with_alice(100);
        testing_env!(context(spender()).build());
        contract.transfer(alice(), NearToken::from_near(60));
        testing_env!(context(treasury()).build());
        contract.on_transfer(alice(), NearToken::from_near(60), Err(PromiseError::Failed));
        assert_eq!(near_remaining(&contract), yocto(100));
    }

    #[test]
    fn test_near_successful_transfer_keeps_limit_consumed() {
        let mut contract = near_contract_with_alice(100);
        testing_env!(context(spender()).build());
        contract.transfer(alice(), NearToken::from_near(60));
        testing_env!(context(treasury()).build());
        contract.on_transfer(alice(), NearToken::from_near(60), Ok(()));
        assert_eq!(near_remaining(&contract), yocto(40));
    }

    #[test]
    fn test_near_rollback_after_readd_tops_up_fresh_limit() {
        let mut contract = near_contract_with_alice(100);
        testing_env!(context(spender()).build());
        contract.transfer(alice(), NearToken::from_near(60));
        // The account is removed and re-added before the failed transfer's
        // callback lands: the refund is added on top of the fresh limit.
        testing_env!(context(manager()).build());
        contract.remove_from_whitelist(keys(alice(), None));
        contract.add_to_whitelist(entries(alice(), None, yocto(100)));
        testing_env!(context(treasury()).build());
        contract.on_transfer(alice(), NearToken::from_near(60), Err(PromiseError::Failed));
        assert_eq!(near_remaining(&contract), yocto(160));
    }

    #[test]
    fn test_near_rollback_after_removal_does_not_panic() {
        let mut contract = near_contract_with_alice(100);
        testing_env!(context(spender()).build());
        contract.transfer(alice(), NearToken::from_near(60));
        testing_env!(context(manager()).build());
        contract.remove_from_whitelist(keys(alice(), None));
        testing_env!(context(treasury()).build());
        contract.on_transfer(alice(), NearToken::from_near(60), Err(PromiseError::Failed));
        assert!(contract.get_whitelist_entry(alice(), None).is_none());
    }

    // NEP-141 token transfers (`token_id: Some` whitelist entries).

    #[test]
    fn test_ft_transfer_within_limit() {
        let mut contract = ft_contract_with_alice(FT_LIMIT);
        testing_env!(context(spender()).build());
        contract.transfer_ft(alice(), token(), U128(60));
        contract.transfer_ft(alice(), token(), U128(40));
        assert_eq!(ft_remaining(&contract), 0);
    }

    #[test]
    #[should_panic(expected = "Transfer exceeds the limit")]
    fn test_ft_transfer_over_limit_fails() {
        let mut contract = ft_contract_with_alice(FT_LIMIT);
        testing_env!(context(spender()).build());
        contract.transfer_ft(alice(), token(), U128(FT_LIMIT + 1));
    }

    #[test]
    #[should_panic(expected = "Transfer exceeds the limit")]
    fn test_ft_transfer_cumulative_over_limit_fails() {
        let mut contract = ft_contract_with_alice(FT_LIMIT);
        testing_env!(context(spender()).build());
        contract.transfer_ft(alice(), token(), U128(60));
        contract.transfer_ft(alice(), token(), U128(41));
    }

    #[test]
    #[should_panic(expected = "Only the spender can transfer")]
    fn test_ft_transfer_by_admin_fails() {
        let mut contract = ft_contract_with_alice(FT_LIMIT);
        testing_env!(context(admin()).build());
        contract.transfer_ft(alice(), token(), U128(1));
    }

    #[test]
    #[should_panic(expected = "Receiver is not whitelisted for this token")]
    fn test_ft_transfer_other_token_fails() {
        let mut contract = ft_contract_with_alice(FT_LIMIT);
        testing_env!(context(spender()).build());
        contract.transfer_ft(alice(), other_token(), U128(1));
    }

    #[test]
    #[should_panic(expected = "Receiver is not whitelisted for this token")]
    fn test_ft_near_whitelist_does_not_grant_ft() {
        testing_env!(context(manager()).build());
        let mut contract = new_contract();
        contract.add_to_whitelist(entries(
            alice(),
            None,
            U128(NearToken::from_near(100).as_yoctonear()),
        ));
        testing_env!(context(spender()).build());
        contract.transfer_ft(alice(), token(), U128(1));
    }

    #[test]
    #[should_panic(expected = "Amount must be positive")]
    fn test_ft_transfer_zero_fails() {
        let mut contract = ft_contract_with_alice(FT_LIMIT);
        testing_env!(context(spender()).build());
        contract.transfer_ft(alice(), token(), U128(0));
    }

    #[test]
    fn test_ft_raised_limit_extends_allowance() {
        let mut contract = ft_contract_with_alice(FT_LIMIT);
        testing_env!(context(spender()).build());
        contract.transfer_ft(alice(), token(), U128(FT_LIMIT));
        // The allowance is used up; the manager must raise it to allow more.
        testing_env!(context(manager()).build());
        contract.increase_limit(changes(alice(), Some(token()), U128(50)));
        testing_env!(context(spender()).build());
        contract.transfer_ft(alice(), token(), U128(50));
        assert_eq!(ft_remaining(&contract), 0);
    }

    #[test]
    fn test_ft_failed_transfer_restores_limit() {
        let mut contract = ft_contract_with_alice(FT_LIMIT);
        testing_env!(context(spender()).build());
        contract.transfer_ft(alice(), token(), U128(60));
        testing_env!(context(treasury()).build());
        contract.on_transfer_ft(alice(), token(), U128(60), Err(PromiseError::Failed));
        assert_eq!(ft_remaining(&contract), FT_LIMIT);
    }

    #[test]
    fn test_ft_successful_transfer_keeps_limit_consumed() {
        let mut contract = ft_contract_with_alice(FT_LIMIT);
        testing_env!(context(spender()).build());
        contract.transfer_ft(alice(), token(), U128(60));
        testing_env!(context(treasury()).build());
        contract.on_transfer_ft(alice(), token(), U128(60), Ok(()));
        assert_eq!(ft_remaining(&contract), FT_LIMIT - 60);
    }

    #[test]
    fn test_ft_rollback_after_readd_tops_up_fresh_limit() {
        let mut contract = ft_contract_with_alice(FT_LIMIT);
        testing_env!(context(spender()).build());
        contract.transfer_ft(alice(), token(), U128(60));
        // The pair is removed and re-added before the failed transfer's
        // callback lands: the refund is added on top of the fresh limit.
        testing_env!(context(manager()).build());
        contract.remove_from_whitelist(keys(alice(), Some(token())));
        contract.add_to_whitelist(entries(alice(), Some(token()), U128(FT_LIMIT)));
        testing_env!(context(treasury()).build());
        contract.on_transfer_ft(alice(), token(), U128(60), Err(PromiseError::Failed));
        assert_eq!(ft_remaining(&contract), FT_LIMIT + 60);
    }

    #[test]
    fn test_ft_rollback_after_removal_does_not_panic() {
        let mut contract = ft_contract_with_alice(FT_LIMIT);
        testing_env!(context(spender()).build());
        contract.transfer_ft(alice(), token(), U128(60));
        testing_env!(context(manager()).build());
        contract.remove_from_whitelist(keys(alice(), Some(token())));
        testing_env!(context(treasury()).build());
        contract.on_transfer_ft(alice(), token(), U128(60), Err(PromiseError::Failed));
        assert!(
            contract
                .get_whitelist_entry(alice(), Some(token()))
                .is_none()
        );
    }
}
