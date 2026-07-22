//! Helpers for deploying a NEP-141 token (`w_near.wasm`, mintable by
//! depositing NEAR) and calling the treasury's FT methods.

use crate::setup::{TreasuryTestWorkspace, outcome_check};
use near_sdk::NearToken;
use near_sdk::json_types::U128;
use near_workspaces::result::ExecutionFinalResult;
use near_workspaces::{Account, AccountId};
use serde_json::json;

pub const FT_WASM_FILEPATH: &str = "../res/w_near.wasm";

#[allow(dead_code)]
impl TreasuryTestWorkspace {
    /// Deploys the wNEAR test token, registers the treasury with it and mints
    /// `treasury_balance` onto the treasury's token balance (wNEAR is 1:1 with
    /// NEAR, so token amounts are expressed as `NearToken`).
    pub async fn deploy_ft_and_fund_treasury(
        &self,
        treasury_balance: NearToken,
    ) -> Result<Account, Box<dyn std::error::Error>> {
        let ft_wasm = std::fs::read(FT_WASM_FILEPATH)?;
        let token = self.sandbox.dev_create_account().await?;
        let outcome = token
            .batch(token.id())
            .deploy(&ft_wasm)
            .call(near_workspaces::operations::Function::new("new"))
            .transact()
            .await?;
        assert!(
            outcome.is_success(),
            "Failed to deploy the FT contract: {:#?}",
            outcome.outcomes()
        );

        // A funder mints wNEAR by depositing NEAR and moves it to the treasury.
        let funder = self.sandbox.dev_create_account().await?;
        self.ft_register(&token, funder.id()).await?;
        self.ft_register(&token, self.treasury.id()).await?;
        outcome_check(
            &funder
                .call(token.id(), "near_deposit")
                .deposit(treasury_balance)
                .transact()
                .await?,
        );
        outcome_check(
            &funder
                .call(token.id(), "ft_transfer")
                .args_json(json!({
                    "receiver_id": self.treasury.id(),
                    "amount": U128(treasury_balance.as_yoctonear()),
                }))
                .deposit(NearToken::from_yoctonear(1))
                .transact()
                .await?,
        );
        Ok(token)
    }

    /// Pays the NEP-145 storage deposit registering `account_id` with the token.
    pub async fn ft_register(
        &self,
        token: &Account,
        account_id: &AccountId,
    ) -> Result<(), Box<dyn std::error::Error>> {
        outcome_check(
            &token
                .call(token.id(), "storage_deposit")
                .args_json(json!({ "account_id": account_id }))
                .deposit(NearToken::from_millinear(10))
                .transact()
                .await?,
        );
        Ok(())
    }

    pub async fn ft_balance(
        &self,
        token: &Account,
        account_id: &AccountId,
    ) -> Result<u128, Box<dyn std::error::Error>> {
        let balance: U128 = self
            .sandbox
            .view(token.id(), "ft_balance_of")
            .args_json(json!({ "account_id": account_id }))
            .await?
            .json()?;
        Ok(balance.0)
    }

    /// Whitelists a (receiver, token) pair via the unified `add_to_whitelist`
    /// method (`token_id: Some`).
    pub async fn add_to_ft_whitelist(
        &self,
        caller: &Account,
        account_id: &AccountId,
        token_id: &AccountId,
        limit: u128,
    ) -> Result<ExecutionFinalResult, Box<dyn std::error::Error>> {
        Ok(caller
            .call(self.treasury.id(), "add_to_whitelist")
            .args_json(json!({
                "account_id": account_id,
                "token_id": token_id,
                "limit": U128(limit),
            }))
            .transact()
            .await?)
    }

    /// Removes a (receiver, token) pair via the unified `remove_from_whitelist`
    /// method (`token_id: Some`).
    pub async fn remove_from_ft_whitelist(
        &self,
        caller: &Account,
        account_id: &AccountId,
        token_id: &AccountId,
    ) -> Result<ExecutionFinalResult, Box<dyn std::error::Error>> {
        Ok(caller
            .call(self.treasury.id(), "remove_from_whitelist")
            .args_json(json!({
                "account_id": account_id,
                "token_id": token_id,
            }))
            .transact()
            .await?)
    }

    /// Sets the FT limit via the unified `set_limit` method (`token_id: Some`).
    pub async fn set_ft_limit(
        &self,
        caller: &Account,
        account_id: &AccountId,
        token_id: &AccountId,
        limit: u128,
    ) -> Result<ExecutionFinalResult, Box<dyn std::error::Error>> {
        Ok(caller
            .call(self.treasury.id(), "set_limit")
            .args_json(json!({
                "account_id": account_id,
                "token_id": token_id,
                "limit": U128(limit),
            }))
            .transact()
            .await?)
    }

    /// Calls `transfer_ft` on the treasury. Even when the underlying `ft_transfer`
    /// fails, the transaction succeeds because the callback absorbs the error;
    /// check `outcome.failures()` to observe the rollback.
    pub async fn transfer_ft(
        &self,
        caller: &Account,
        receiver_id: &AccountId,
        token_id: &AccountId,
        amount: u128,
    ) -> Result<ExecutionFinalResult, Box<dyn std::error::Error>> {
        Ok(caller
            .call(self.treasury.id(), "transfer_ft")
            .args_json(json!({
                "receiver_id": receiver_id,
                "token_id": token_id,
                "amount": U128(amount),
            }))
            .gas(near_sdk::Gas::from_tgas(80))
            .transact()
            .await?)
    }

    pub async fn is_ft_whitelisted(
        &self,
        account_id: &AccountId,
        token_id: &AccountId,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        Ok(!self
            .ft_whitelist_entry(account_id, token_id)
            .await?
            .is_null())
    }

    pub async fn ft_whitelist_entry(
        &self,
        account_id: &AccountId,
        token_id: &AccountId,
    ) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
        self.treasury_view(
            "get_whitelist_entry",
            json!({ "account_id": account_id, "token_id": token_id }),
        )
        .await
    }

    /// Returns the remaining token limit of a whitelisted (receiver, token) pair.
    pub async fn ft_remaining_limit(
        &self,
        account_id: &AccountId,
        token_id: &AccountId,
    ) -> Result<u128, Box<dyn std::error::Error>> {
        let entry = self.ft_whitelist_entry(account_id, token_id).await?;
        assert!(
            !entry.is_null(),
            "Pair is not whitelisted: {account_id} / {token_id}"
        );
        let limit: U128 = serde_json::from_value(entry["limit"].clone())?;
        Ok(limit.0)
    }
}
