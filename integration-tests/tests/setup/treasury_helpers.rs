//! Helpers for calling and viewing the `spending-account` contract directly
//! (as whichever account holds the admin/manager/spender role).

use crate::setup::TreasuryTestWorkspace;
use near_sdk::NearToken;
use near_sdk::json_types::U128;
use near_workspaces::result::ExecutionFinalResult;
use near_workspaces::{Account, AccountId};
use serde_json::json;

#[allow(dead_code)]
impl TreasuryTestWorkspace {
    pub async fn treasury_view(
        &self,
        method: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
        Ok(self
            .sandbox
            .view(self.treasury.id(), method)
            .args_json(args)
            .await?
            .json()?)
    }

    /// Whitelists `account_id` for native NEAR (`token_id: None`).
    pub async fn add_to_whitelist(
        &self,
        caller: &Account,
        account_id: &AccountId,
        limit: NearToken,
    ) -> Result<ExecutionFinalResult, Box<dyn std::error::Error>> {
        Ok(caller
            .call(self.treasury.id(), "add_to_whitelist")
            .args_json(json!({
                "account_id": account_id,
                "token_id": null,
                "limit": U128(limit.as_yoctonear()),
            }))
            .transact()
            .await?)
    }

    /// Removes the native NEAR entry of `account_id` (`token_id: None`).
    pub async fn remove_from_whitelist(
        &self,
        caller: &Account,
        account_id: &AccountId,
    ) -> Result<ExecutionFinalResult, Box<dyn std::error::Error>> {
        Ok(caller
            .call(self.treasury.id(), "remove_from_whitelist")
            .args_json(json!({ "account_id": account_id, "token_id": null }))
            .transact()
            .await?)
    }

    /// Sets the native NEAR limit (`token_id: None` on the unified method).
    pub async fn set_limit(
        &self,
        caller: &Account,
        account_id: &AccountId,
        limit: NearToken,
    ) -> Result<ExecutionFinalResult, Box<dyn std::error::Error>> {
        Ok(caller
            .call(self.treasury.id(), "set_limit")
            .args_json(json!({
                "account_id": account_id,
                "token_id": null,
                "limit": U128(limit.as_yoctonear()),
            }))
            .transact()
            .await?)
    }

    /// Calls `transfer` on the treasury. Note: even when the underlying NEAR
    /// transfer fails, the transaction as a whole succeeds because the
    /// `on_transfer` callback handles the error; check `outcome.failures()`
    /// or the whitelist entry to observe the rollback.
    pub async fn transfer(
        &self,
        caller: &Account,
        receiver_id: &AccountId,
        amount: NearToken,
    ) -> Result<ExecutionFinalResult, Box<dyn std::error::Error>> {
        Ok(caller
            .call(self.treasury.id(), "transfer")
            .args_json(json!({
                "receiver_id": receiver_id,
                "amount": amount,
            }))
            .gas(near_sdk::Gas::from_tgas(50))
            .transact()
            .await?)
    }

    /// Checks whether `account_id` has a native NEAR entry (`token_id: None`).
    pub async fn is_whitelisted(
        &self,
        account_id: &AccountId,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        Ok(!self.whitelist_entry(account_id).await?.is_null())
    }

    pub async fn get_num_whitelisted(&self) -> Result<u32, Box<dyn std::error::Error>> {
        Ok(serde_json::from_value(
            self.treasury_view("get_num_whitelisted", json!({})).await?,
        )?)
    }

    /// Returns the native NEAR entry of `account_id` (`token_id: None`).
    pub async fn whitelist_entry(
        &self,
        account_id: &AccountId,
    ) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
        self.treasury_view(
            "get_whitelist_entry",
            json!({ "account_id": account_id, "token_id": null }),
        )
        .await
    }

    /// Returns the remaining native NEAR limit of a whitelisted account.
    pub async fn remaining_limit(
        &self,
        account_id: &AccountId,
    ) -> Result<NearToken, Box<dyn std::error::Error>> {
        let entry = self.whitelist_entry(account_id).await?;
        assert!(!entry.is_null(), "Account is not whitelisted: {account_id}");
        Ok(serde_json::from_value(entry["limit"].clone())?)
    }
}
