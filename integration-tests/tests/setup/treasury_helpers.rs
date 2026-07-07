//! Helpers for calling and viewing the `spending-account` contract directly
//! (as whichever account holds the admin/spender role).

use crate::setup::TreasuryTestWorkspace;
use near_sdk::NearToken;
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

    pub async fn add_to_whitelist(
        &self,
        caller: &Account,
        account_id: &AccountId,
        yearly_limit: NearToken,
    ) -> Result<ExecutionFinalResult, Box<dyn std::error::Error>> {
        Ok(caller
            .call(self.treasury.id(), "add_to_whitelist")
            .args_json(json!({
                "account_id": account_id,
                "yearly_limit": yearly_limit,
            }))
            .transact()
            .await?)
    }

    pub async fn remove_from_whitelist(
        &self,
        caller: &Account,
        account_id: &AccountId,
    ) -> Result<ExecutionFinalResult, Box<dyn std::error::Error>> {
        Ok(caller
            .call(self.treasury.id(), "remove_from_whitelist")
            .args_json(json!({ "account_id": account_id }))
            .transact()
            .await?)
    }

    pub async fn set_yearly_limit(
        &self,
        caller: &Account,
        account_id: &AccountId,
        yearly_limit: NearToken,
    ) -> Result<ExecutionFinalResult, Box<dyn std::error::Error>> {
        Ok(caller
            .call(self.treasury.id(), "set_yearly_limit")
            .args_json(json!({
                "account_id": account_id,
                "yearly_limit": yearly_limit,
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

    pub async fn is_whitelisted(
        &self,
        account_id: &AccountId,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        Ok(serde_json::from_value(
            self.treasury_view("is_whitelisted", json!({ "account_id": account_id }))
                .await?,
        )?)
    }

    pub async fn get_num_whitelisted(&self) -> Result<u32, Box<dyn std::error::Error>> {
        Ok(serde_json::from_value(
            self.treasury_view("get_num_whitelisted", json!({})).await?,
        )?)
    }

    pub async fn whitelist_entry(
        &self,
        account_id: &AccountId,
    ) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
        self.treasury_view("get_whitelist_entry", json!({ "account_id": account_id }))
            .await
    }

    /// Returns the (spent, available) amounts of a whitelisted account for the
    /// current period.
    pub async fn spent_and_available(
        &self,
        account_id: &AccountId,
    ) -> Result<(NearToken, NearToken), Box<dyn std::error::Error>> {
        let entry = self.whitelist_entry(account_id).await?;
        assert!(!entry.is_null(), "Account is not whitelisted: {account_id}");
        let spent = serde_json::from_value(entry["spent"].clone())?;
        let available = serde_json::from_value(entry["available"].clone())?;
        Ok((spent, available))
    }

    pub async fn period_info(&self) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
        self.treasury_view("get_period_info", json!({})).await
    }

    pub async fn period_index(&self) -> Result<u64, Box<dyn std::error::Error>> {
        Ok(self.period_info().await?["period_index"].as_u64().unwrap())
    }
}
