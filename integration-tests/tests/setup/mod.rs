pub mod ft_helpers;
pub mod sputnik_helpers;
pub mod timelock_helpers;
pub mod treasury_helpers;

use near_sdk::json_types::U64;
use near_sdk::{NearToken, Timestamp};
use near_workspaces::network::Sandbox;
use near_workspaces::operations::Function;
use near_workspaces::{Account, Worker};
use serde_json::json;

#[allow(dead_code)]
pub const NS_IN_SECOND: u64 = 1_000_000_000;
#[allow(dead_code)]
pub const TIMELOCK_DELAY_SECONDS: u64 = 60;

pub const TIMELOCK_WASM_FILEPATH: &str = "../res/local/dao_timelock.wasm";
pub const SPENDING_ACCOUNT_WASM_FILEPATH: &str = "../res/local/spending_account.wasm";

/// A deployed treasury stack.
///
/// The spending account is always deployed. When the builder is configured
/// `with_timelocks`, a `dao-timelock` is deployed in front of each role: the
/// admin/spender of the treasury are the timelock accounts and the DAOs act
/// through them. Otherwise the DAO accounts hold the roles directly, which is
/// convenient for testing the spending account in isolation.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct TreasuryTestWorkspace {
    pub sandbox: Worker<Sandbox>,
    /// The spending-account contract.
    pub treasury: Account,
    /// The account acting as the admin DAO.
    pub admin_dao: Account,
    /// The account acting as the spender DAO.
    pub spender_dao: Account,
    /// Timelock controlled by `admin_dao`, holding the treasury admin role and
    /// the admin role of both timelocks (its own included).
    pub admin_timelock: Option<Account>,
    /// Timelock controlled by `spender_dao`, holding the treasury spender role.
    pub spender_timelock: Option<Account>,
    /// Guardian of both timelocks.
    pub guardian: Account,
}

#[derive(Clone, Debug)]
pub struct TreasuryTestWorkspaceBuilder {
    pub delay_ns: u64,
    pub deploy_timelocks: bool,
    pub treasury_funding: NearToken,
}

impl Default for TreasuryTestWorkspaceBuilder {
    fn default() -> Self {
        Self {
            delay_ns: TIMELOCK_DELAY_SECONDS * NS_IN_SECOND,
            deploy_timelocks: false,
            treasury_funding: NearToken::from_near(1000),
        }
    }
}

#[allow(dead_code)]
impl TreasuryTestWorkspaceBuilder {
    pub async fn build(self) -> Result<TreasuryTestWorkspace, Box<dyn std::error::Error>> {
        let spending_account_wasm = std::fs::read(SPENDING_ACCOUNT_WASM_FILEPATH)?;

        let sandbox = near_workspaces::sandbox().await?;

        let admin_dao = sandbox.dev_create_account().await?;
        let spender_dao = sandbox.dev_create_account().await?;
        let guardian = sandbox.dev_create_account().await?;

        let (admin_timelock, spender_timelock) = if self.deploy_timelocks {
            let timelock_wasm = std::fs::read(TIMELOCK_WASM_FILEPATH)?;
            let admin_timelock = sandbox.dev_create_account().await?;
            let spender_timelock = sandbox.dev_create_account().await?;
            // The admin timelock administers every timelock, including itself:
            // its own config changes go through its own scheduled requests.
            for (timelock, dao) in [
                (&admin_timelock, &admin_dao),
                (&spender_timelock, &spender_dao),
            ] {
                let outcome = timelock
                    .batch(timelock.id())
                    .deploy(&timelock_wasm)
                    .call(
                        Function::new("new")
                            .args_json(json!({
                                "dao_id": dao.id(),
                                "admin_id": admin_timelock.id(),
                                "guardians": &[guardian.id()],
                                "delay_ns": U64(self.delay_ns),
                            }))
                            .gas(near_sdk::Gas::from_tgas(10)),
                    )
                    .transact()
                    .await?;
                assert!(
                    outcome.is_success(),
                    "Failed to deploy dao-timelock: {:#?}",
                    outcome.outcomes()
                );
            }
            (Some(admin_timelock), Some(spender_timelock))
        } else {
            (None, None)
        };

        let admin_id = admin_timelock.as_ref().unwrap_or(&admin_dao).id();
        let spender_id = spender_timelock.as_ref().unwrap_or(&spender_dao).id();

        let treasury = sandbox.dev_create_account().await?;
        let outcome = treasury
            .batch(treasury.id())
            .deploy(&spending_account_wasm)
            .call(
                Function::new("new")
                    .args_json(json!({
                        "admin_id": admin_id,
                        "spender_id": spender_id,
                    }))
                    .gas(near_sdk::Gas::from_tgas(10)),
            )
            .transact()
            .await?;
        assert!(
            outcome.is_success(),
            "Failed to deploy spending-account: {:#?}",
            outcome.outcomes()
        );

        // Fund the treasury from the sandbox root account.
        let outcome = sandbox
            .root_account()
            .unwrap()
            .transfer_near(treasury.id(), self.treasury_funding)
            .await?;
        outcome_check(&outcome);

        let workspace = TreasuryTestWorkspace {
            sandbox,
            treasury,
            admin_dao,
            spender_dao,
            admin_timelock,
            spender_timelock,
            guardian,
        };

        // Sanity check the deployed configuration.
        let configured_admin: near_workspaces::AccountId = workspace
            .treasury_view("get_admin", json!({}))
            .await
            .map(|v| serde_json::from_value(v).unwrap())?;
        assert_eq!(&configured_admin, workspace.admin().id(), "Invalid admin");
        let configured_spender: near_workspaces::AccountId = workspace
            .treasury_view("get_spender", json!({}))
            .await
            .map(|v| serde_json::from_value(v).unwrap())?;
        assert_eq!(
            &configured_spender,
            workspace.spender().id(),
            "Invalid spender"
        );

        Ok(workspace)
    }

    pub fn with_timelocks(mut self) -> Self {
        self.deploy_timelocks = true;
        self
    }

    pub fn delay_ns(mut self, delay_ns: u64) -> Self {
        self.delay_ns = delay_ns;
        self
    }

    pub fn treasury_funding(mut self, treasury_funding: NearToken) -> Self {
        self.treasury_funding = treasury_funding;
        self
    }
}

#[allow(dead_code)]
impl TreasuryTestWorkspace {
    /// The account holding the treasury admin role: the admin timelock when
    /// deployed, otherwise the admin DAO itself.
    pub fn admin(&self) -> &Account {
        self.admin_timelock.as_ref().unwrap_or(&self.admin_dao)
    }

    /// The account holding the treasury spender role: the spender timelock when
    /// deployed, otherwise the spender DAO itself.
    pub fn spender(&self) -> &Account {
        self.spender_timelock.as_ref().unwrap_or(&self.spender_dao)
    }

    pub async fn block_timestamp(&self) -> Result<Timestamp, Box<dyn std::error::Error>> {
        Ok(self.sandbox.view_block().await?.timestamp())
    }

    pub async fn balance(
        &self,
        account: &Account,
    ) -> Result<NearToken, Box<dyn std::error::Error>> {
        Ok(self.sandbox.view_account(account.id()).await?.balance)
    }

    pub async fn fast_forward(
        &self,
        timestamp: Timestamp,
        num_block: u64,
        max_num_iterations: usize,
    ) -> Result<(), Box<dyn std::error::Error>> {
        for i in 1..=max_num_iterations {
            self.sandbox.fast_forward(num_block).await?;
            let block = self.sandbox.view_block().await?;
            if block.timestamp() >= timestamp {
                break;
            } else {
                assert_ne!(i, max_num_iterations, "Target timestamp was not reached");
            }
        }

        Ok(())
    }
}

#[track_caller]
#[allow(dead_code)]
pub fn outcome_check(outcome: &near_workspaces::result::ExecutionFinalResult) {
    if outcome.failures().len() > 0 || outcome.is_failure() {
        println!("Failure outcome: {:?}", &outcome);
    }
    assert!(outcome.failures().len() == 0 && outcome.is_success());
}

#[track_caller]
#[allow(dead_code)]
pub fn assert_almost_eq(left: NearToken, right: NearToken, max_delta: NearToken) {
    let left2 = left.as_yoctonear();
    let right2 = right.as_yoctonear();
    let max_delta2 = max_delta.as_yoctonear();
    assert!(
        std::cmp::max(left2, right2) - std::cmp::min(left2, right2) <= max_delta2,
        "{}",
        format!(
            "Left {} is not even close to Right {} within delta {}",
            left.exact_amount_display(),
            right.exact_amount_display(),
            max_delta.exact_amount_display()
        )
    );
}
