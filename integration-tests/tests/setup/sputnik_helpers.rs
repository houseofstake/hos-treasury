//! Helpers for deploying a real SputnikDAO v2 (`res/sputnikdao2.wasm`, v2.3.1)
//! in front of a `dao-timelock` and driving it through proposals.
//!
//! Topology under test:
//!
//! ```text
//! council (2-of-3) -> sputnik DAO -> dao-timelock -> target (incl. the DAO itself)
//! ```
//!
//! The DAO policy is configured so that policy upgrades can only happen through
//! the timelock:
//!   - the `council` role (3 members, default 1/2 ratio => 2 approvals) can only
//!     add and vote on `FunctionCall` ("call") proposals;
//!   - the `timelock` role (the timelock account alone, threshold 1) is the only
//!     role with `policy:AddProposal` / `policy:VoteApprove` permissions.
//!
//! A policy upgrade therefore flows as: the council approves a `FunctionCall`
//! proposal that calls `timelock.schedule_proposal` with the `ChangePolicy`
//! kind. After the timelock delay anyone can execute it: the timelock submits
//! the inner proposal via `add_proposal` and approves the id the DAO returned
//! in a callback, so proposals added by others during the delay cannot break
//! the flow by shifting the id sequence.

use crate::setup::{NS_IN_SECOND, TIMELOCK_DELAY_SECONDS, TIMELOCK_WASM_FILEPATH};
use near_sdk::json_types::{Base64VecU8, U64};
use near_sdk::{Gas, NearToken, Timestamp};
use near_workspaces::network::Sandbox;
use near_workspaces::operations::Function;
use near_workspaces::result::ExecutionFinalResult;
use near_workspaces::{Account, AccountId, Worker};
use serde_json::json;

#[allow(dead_code)]
pub const SPUTNIK_WASM_FILEPATH: &str = "../res/sputnikdao2.wasm";

/// Proposal bond of the initial DAO policy.
#[allow(dead_code)]
pub const PROPOSAL_BOND: NearToken = NearToken::from_near(1);

/// A deployed SputnikDAO + dao-timelock pair.
#[allow(dead_code)]
pub struct SputnikTimelockWorkspace {
    pub sandbox: Worker<Sandbox>,
    /// The SputnikDAO v2 contract.
    pub dao: Account,
    /// The dao-timelock contract, configured with the DAO as its `dao_id`.
    pub timelock: Account,
    /// The three council members. Any 2-of-3 approvals pass a proposal.
    pub council: Vec<Account>,
    /// Guardian of the timelock.
    pub guardian: Account,
}

/// Builds the DAO policy JSON: a 2-of-3 council that can only act on
/// `FunctionCall` proposals, and the timelock as the only account allowed to
/// add and approve `ChangePolicy` proposals.
#[allow(dead_code)]
pub fn dao_policy(
    council: &[Account],
    timelock_id: &AccountId,
    proposal_bond: NearToken,
) -> serde_json::Value {
    json!({
        "roles": [
            {
                "name": "council",
                "kind": { "Group": council.iter().map(|a| a.id()).collect::<Vec<_>>() },
                // No "policy:*" permissions: the council cannot touch the policy
                // directly, every upgrade must go through the timelock.
                "permissions": [
                    "call:AddProposal",
                    "call:VoteApprove",
                    "call:VoteReject",
                    "call:VoteRemove",
                ],
                "vote_policy": {},
            },
            {
                "name": "timelock",
                "kind": { "Group": [timelock_id] },
                // With a single member and the default 1/2 ratio, one vote by
                // the timelock approves a policy proposal.
                "permissions": ["policy:AddProposal", "policy:VoteApprove"],
                "vote_policy": {},
            },
        ],
        // RoleWeight with ratio 1/2 => floor(n/2) + 1 approvals: 2-of-3 for the council.
        "default_vote_policy": { "weight_kind": "RoleWeight", "quorum": "0", "threshold": [1, 2] },
        "proposal_bond": proposal_bond,
        "proposal_period": U64(7 * 24 * 60 * 60 * NS_IN_SECOND),
        "bounty_bond": NearToken::from_near(1),
        "bounty_forgiveness_period": U64(24 * 60 * 60 * NS_IN_SECOND),
    })
}

/// The `ChangePolicy` proposal kind for the given policy JSON.
#[allow(dead_code)]
pub fn change_policy_kind(new_policy: &serde_json::Value) -> serde_json::Value {
    json!({ "ChangePolicy": { "policy": new_policy } })
}

/// The council-facing `FunctionCall` proposal kind that asks the timelock to
/// schedule an inner proposal on the DAO. Once the scheduled request is
/// executed, the timelock adds the proposal and approves whatever id the DAO
/// assigned to it with its own vote.
///
/// The action deposit equals the proposal bond: the DAO escrows it in the
/// timelock at schedule time and the timelock attaches it as the bond of the
/// inner `add_proposal`.
#[allow(dead_code)]
pub fn schedule_proposal_kind(
    timelock_id: &AccountId,
    inner_kind: &serde_json::Value,
    proposal_bond: NearToken,
) -> serde_json::Value {
    let schedule_args = json!({
        "description": "Scheduled policy change",
        "kind": inner_kind,
        "add_proposal_gas": Gas::from_tgas(50),
        "act_proposal_gas": Gas::from_tgas(100),
    });
    json!({
        "FunctionCall": {
            "receiver_id": timelock_id,
            "actions": [{
                "method_name": "schedule_proposal",
                "args": Base64VecU8::from(serde_json::to_vec(&schedule_args).unwrap()),
                "deposit": proposal_bond,
                "gas": Gas::from_tgas(50),
            }],
        }
    })
}

/// `schedule_proposal_kind` for a `ChangePolicy` inner proposal.
#[allow(dead_code)]
pub fn schedule_policy_change_kind(
    timelock_id: &AccountId,
    new_policy: &serde_json::Value,
    proposal_bond: NearToken,
) -> serde_json::Value {
    schedule_proposal_kind(timelock_id, &change_policy_kind(new_policy), proposal_bond)
}

/// Deploys the timelock (with the DAO as `dao_id`) and the SputnikDAO (with the
/// 2-of-3 council policy above).
#[allow(dead_code)]
pub async fn setup_sputnik_workspace()
-> Result<SputnikTimelockWorkspace, Box<dyn std::error::Error>> {
    let sandbox = near_workspaces::sandbox().await?;

    let mut council = Vec::new();
    for _ in 0..3 {
        council.push(sandbox.dev_create_account().await?);
    }
    let guardian = sandbox.dev_create_account().await?;
    let dao = sandbox.dev_create_account().await?;
    let timelock = sandbox.dev_create_account().await?;

    let timelock_wasm = std::fs::read(TIMELOCK_WASM_FILEPATH)?;
    let outcome = timelock
        .batch(timelock.id())
        .deploy(&timelock_wasm)
        .call(
            Function::new("new")
                .args_json(json!({
                    "dao_id": dao.id(),
                    "admin_id": timelock.id(),
                    "guardians": [guardian.id()],
                    "delay_ns": U64(TIMELOCK_DELAY_SECONDS * NS_IN_SECOND),
                }))
                .gas(Gas::from_tgas(10)),
        )
        .transact()
        .await?;
    assert!(
        outcome.is_success(),
        "Failed to deploy dao-timelock: {:#?}",
        outcome.outcomes()
    );

    let sputnik_wasm = std::fs::read(SPUTNIK_WASM_FILEPATH)?;
    let outcome = dao
        .batch(dao.id())
        .deploy(&sputnik_wasm)
        .call(
            Function::new("new")
                .args_json(json!({
                    "config": {
                        "name": "treasury-dao",
                        "purpose": "hos-treasury integration tests",
                        "metadata": "",
                    },
                    "policy": dao_policy(&council, timelock.id(), PROPOSAL_BOND),
                }))
                .gas(Gas::from_tgas(100)),
        )
        .transact()
        .await?;
    assert!(
        outcome.is_success(),
        "Failed to deploy sputnikdao2: {:#?}",
        outcome.outcomes()
    );

    Ok(SputnikTimelockWorkspace {
        sandbox,
        dao,
        timelock,
        council,
        guardian,
    })
}

#[allow(dead_code)]
impl SputnikTimelockWorkspace {
    // ---- DAO (sputnikdao2) helpers ----

    /// Adds a proposal without asserting the outcome.
    pub async fn add_proposal_raw(
        &self,
        proposer: &Account,
        kind: &serde_json::Value,
        bond: NearToken,
    ) -> Result<ExecutionFinalResult, Box<dyn std::error::Error>> {
        Ok(proposer
            .call(self.dao.id(), "add_proposal")
            .args_json(json!({
                "proposal": {
                    "description": "integration test proposal",
                    "kind": kind,
                }
            }))
            .deposit(bond)
            .gas(Gas::from_tgas(100))
            .transact()
            .await?)
    }

    /// Adds a proposal with the given bond and returns its id.
    pub async fn add_proposal(
        &self,
        proposer: &Account,
        kind: &serde_json::Value,
        bond: NearToken,
    ) -> Result<u64, Box<dyn std::error::Error>> {
        let outcome = self.add_proposal_raw(proposer, kind, bond).await?;
        assert!(
            outcome.is_success(),
            "Failed to add proposal: {:#?}",
            outcome.outcomes()
        );
        Ok(outcome.unwrap().json()?)
    }

    /// Votes on a proposal without asserting the outcome. The v2.3.1 ABI
    /// requires re-submitting the proposal kind so the DAO can verify the
    /// voter acts on the proposal they think they act on.
    pub async fn act_proposal(
        &self,
        voter: &Account,
        proposal_id: u64,
        action: &str,
        kind: &serde_json::Value,
    ) -> Result<ExecutionFinalResult, Box<dyn std::error::Error>> {
        Ok(voter
            .call(self.dao.id(), "act_proposal")
            .args_json(json!({
                "id": proposal_id,
                "action": action,
                "proposal": kind,
            }))
            .gas(Gas::from_tgas(300))
            .transact()
            .await?)
    }

    pub async fn get_last_proposal_id(&self) -> Result<u64, Box<dyn std::error::Error>> {
        Ok(self
            .sandbox
            .view(self.dao.id(), "get_last_proposal_id")
            .args_json(json!({}))
            .await?
            .json()?)
    }

    pub async fn get_proposal_status(
        &self,
        proposal_id: u64,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let proposal: serde_json::Value = self
            .sandbox
            .view(self.dao.id(), "get_proposal")
            .args_json(json!({ "id": proposal_id }))
            .await?
            .json()?;
        Ok(proposal["status"]
            .as_str()
            .expect("Missing status")
            .to_string())
    }

    pub async fn get_policy(&self) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
        Ok(self
            .sandbox
            .view(self.dao.id(), "get_policy")
            .args_json(json!({}))
            .await?
            .json()?)
    }

    // ---- Timelock helpers ----

    pub async fn get_num_requests(&self) -> Result<u32, Box<dyn std::error::Error>> {
        Ok(self
            .sandbox
            .view(self.timelock.id(), "get_num_requests")
            .args_json(json!({}))
            .await?
            .json()?)
    }

    pub async fn get_request(
        &self,
        request_id: u64,
    ) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
        Ok(self
            .sandbox
            .view(self.timelock.id(), "get_request")
            .args_json(json!({ "request_id": request_id }))
            .await?
            .json()?)
    }

    pub async fn execute_request(
        &self,
        caller: &Account,
        request_id: u64,
    ) -> Result<ExecutionFinalResult, Box<dyn std::error::Error>> {
        Ok(caller
            .call(self.timelock.id(), "execute")
            .args_json(json!({ "request_id": request_id }))
            .gas(Gas::from_tgas(300))
            .transact()
            .await?)
    }

    // ---- Sandbox helpers ----

    pub async fn balance(
        &self,
        account: &Account,
    ) -> Result<NearToken, Box<dyn std::error::Error>> {
        Ok(self.sandbox.view_account(account.id()).await?.balance)
    }

    pub async fn fast_forward_to(
        &self,
        timestamp: Timestamp,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let max_num_iterations = 20;
        for i in 1..=max_num_iterations {
            let now = self.sandbox.view_block().await?.timestamp();
            if now >= timestamp {
                return Ok(());
            }
            let num_blocks = timestamp.saturating_sub(now) / NS_IN_SECOND + 1;
            self.sandbox.fast_forward(num_blocks).await?;
            assert_ne!(i, max_num_iterations, "Target timestamp was not reached");
        }
        Ok(())
    }

    /// Fast-forwards the sandbox until the request's `execute_after` has passed.
    pub async fn fast_forward_to_executable(
        &self,
        request_id: u64,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let request = self.get_request(request_id).await?;
        assert!(!request.is_null(), "Request not found: {request_id}");
        let execute_after: u64 = request["execute_after"].as_str().unwrap().parse()?;
        self.fast_forward_to(execute_after).await
    }

    // ---- Composite flows ----

    /// The council path for scheduling an inner proposal: proposes a
    /// `FunctionCall` that calls `timelock.schedule_proposal` and approves it
    /// with 2-of-3 votes. Returns the id of the pending timelock request.
    pub async fn schedule_proposal_via_council(
        &self,
        inner_kind: &serde_json::Value,
    ) -> Result<u64, Box<dyn std::error::Error>> {
        let outer_kind = schedule_proposal_kind(self.timelock.id(), inner_kind, PROPOSAL_BOND);

        let proposal_id = self
            .add_proposal(&self.council[0], &outer_kind, PROPOSAL_BOND)
            .await?;

        let timelock_request_id: u64 = self
            .sandbox
            .view(self.timelock.id(), "get_next_request_id")
            .args_json(json!({}))
            .await?
            .json()?;

        for voter in &self.council[..2] {
            let outcome = self
                .act_proposal(voter, proposal_id, "VoteApprove", &outer_kind)
                .await?;
            assert!(
                outcome.is_success() && outcome.failures().is_empty(),
                "Vote failed: {:#?}",
                outcome.outcomes()
            );
        }
        assert_eq!(self.get_proposal_status(proposal_id).await?, "Approved");

        // The approved proposal called `timelock.schedule_proposal`.
        let request = self.get_request(timelock_request_id).await?;
        assert!(
            !request.is_null(),
            "The approved proposal should have scheduled a timelock request"
        );
        Ok(timelock_request_id)
    }

    /// `schedule_proposal_via_council` for a `ChangePolicy` inner proposal.
    pub async fn schedule_policy_change_via_council(
        &self,
        new_policy: &serde_json::Value,
    ) -> Result<u64, Box<dyn std::error::Error>> {
        self.schedule_proposal_via_council(&change_policy_kind(new_policy))
            .await
    }
}
