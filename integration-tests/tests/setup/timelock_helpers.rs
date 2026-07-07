//! Helpers for driving a `dao-timelock`: scheduling requests as the DAO,
//! fast-forwarding past the delay and executing them.

use crate::setup::{NS_IN_SECOND, TreasuryTestWorkspace};
use near_sdk::json_types::Base64VecU8;
use near_sdk::{Gas, NearToken};
use near_workspaces::result::ExecutionFinalResult;
use near_workspaces::{Account, AccountId};
use serde_json::json;

/// Builds the JSON form of a timelock `FunctionCall` action. `args` is the
/// JSON arguments object of the target method, encoded to base64 like a
/// Sputnik `ActionCall`.
#[allow(dead_code)]
pub fn function_call(
    method_name: &str,
    args: serde_json::Value,
    deposit: NearToken,
    tgas: u64,
) -> serde_json::Value {
    json!({
        "method_name": method_name,
        "args": Base64VecU8::from(serde_json::to_vec(&args).unwrap()),
        "deposit": deposit,
        "gas": Gas::from_tgas(tgas),
    })
}

#[allow(dead_code)]
impl TreasuryTestWorkspace {
    /// Schedules a request without asserting the outcome.
    pub async fn schedule_raw(
        &self,
        dao: &Account,
        timelock: &Account,
        receiver_id: &AccountId,
        actions: Vec<serde_json::Value>,
        deposit: NearToken,
    ) -> Result<ExecutionFinalResult, Box<dyn std::error::Error>> {
        Ok(dao
            .call(timelock.id(), "schedule")
            .args_json(json!({
                "receiver_id": receiver_id,
                "actions": actions,
            }))
            .deposit(deposit)
            .gas(Gas::from_tgas(50))
            .transact()
            .await?)
    }

    /// Schedules a request as the DAO and returns the new request id.
    pub async fn schedule(
        &self,
        dao: &Account,
        timelock: &Account,
        receiver_id: &AccountId,
        actions: Vec<serde_json::Value>,
        deposit: NearToken,
    ) -> Result<u64, Box<dyn std::error::Error>> {
        let outcome = self
            .schedule_raw(dao, timelock, receiver_id, actions, deposit)
            .await?;
        assert!(
            outcome.is_success(),
            "Failed to schedule request: {:#?}",
            outcome.outcomes()
        );
        Ok(outcome.unwrap().json()?)
    }

    /// Executes a request without asserting the outcome.
    pub async fn execute(
        &self,
        caller: &Account,
        timelock: &Account,
        request_id: u64,
    ) -> Result<ExecutionFinalResult, Box<dyn std::error::Error>> {
        Ok(caller
            .call(timelock.id(), "execute")
            .args_json(json!({ "request_id": request_id }))
            .gas(Gas::from_tgas(300))
            .transact()
            .await?)
    }

    /// Cancels a request without asserting the outcome.
    pub async fn cancel(
        &self,
        caller: &Account,
        timelock: &Account,
        request_id: u64,
    ) -> Result<ExecutionFinalResult, Box<dyn std::error::Error>> {
        Ok(caller
            .call(timelock.id(), "cancel")
            .args_json(json!({ "request_id": request_id }))
            .transact()
            .await?)
    }

    /// Fast-forwards the sandbox until the request's `execute_after` has passed.
    pub async fn fast_forward_to_executable(
        &self,
        timelock: &Account,
        request_id: u64,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let request = self.get_request(timelock, request_id).await?;
        assert!(!request.is_null(), "Request not found: {request_id}");
        let execute_after: u64 = request["execute_after"].as_str().unwrap().parse()?;
        let now = self.block_timestamp().await?;
        let num_blocks = execute_after.saturating_sub(now) / NS_IN_SECOND + 1;
        self.fast_forward(execute_after, num_blocks, 20).await
    }

    /// The canonical DAO action path: schedule a single function call through
    /// the timelock, wait out the delay and execute it. Returns the execution
    /// outcome so tests can assert success or failure of the inner call.
    pub async fn dao_action(
        &self,
        dao: &Account,
        timelock: &Account,
        receiver_id: &AccountId,
        method_name: &str,
        args: serde_json::Value,
        deposit: NearToken,
        tgas: u64,
    ) -> Result<ExecutionFinalResult, Box<dyn std::error::Error>> {
        let action = function_call(method_name, args, deposit, tgas);
        let request_id = self
            .schedule(dao, timelock, receiver_id, vec![action], deposit)
            .await?;
        self.fast_forward_to_executable(timelock, request_id).await?;
        self.execute(dao, timelock, request_id).await
    }

    pub async fn get_request(
        &self,
        timelock: &Account,
        request_id: u64,
    ) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
        Ok(self
            .sandbox
            .view(timelock.id(), "get_request")
            .args_json(json!({ "request_id": request_id }))
            .await?
            .json()?)
    }

    pub async fn get_requests(
        &self,
        timelock: &Account,
    ) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
        Ok(self
            .sandbox
            .view(timelock.id(), "get_requests")
            .args_json(json!({}))
            .await?
            .json()?)
    }

    pub async fn get_num_requests(
        &self,
        timelock: &Account,
    ) -> Result<u32, Box<dyn std::error::Error>> {
        Ok(self
            .sandbox
            .view(timelock.id(), "get_num_requests")
            .args_json(json!({}))
            .await?
            .json()?)
    }

    pub async fn get_delay_ns(
        &self,
        timelock: &Account,
    ) -> Result<u64, Box<dyn std::error::Error>> {
        let delay: String = self
            .sandbox
            .view(timelock.id(), "get_delay")
            .args_json(json!({}))
            .await?
            .json()?;
        Ok(delay.parse()?)
    }
}
