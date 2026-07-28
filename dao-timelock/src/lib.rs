use near_sdk::json_types::{Base64VecU8, U64};
use near_sdk::store::{IterableMap, IterableSet};
use near_sdk::{
    AccountId, BorshStorageKey, Gas, NearToken, PanicOnDefault, Promise, PromiseError,
    PromiseOrValue, env, near, require,
};

mod admin;
mod events;
mod request;

pub use request::*;

/// Upper bound for the execution delay (30 days in ns)
pub const MAX_DELAY_NS: u64 = 30 * 24 * 60 * 60 * 1_000_000_000;

/// Gas reserved for the `on_proposal_added` callback itself.
pub const GAS_FOR_ON_PROPOSAL_ADDED: Gas = Gas::from_tgas(10);

/// Runtime fee per function call action (`function_call_cost`: 0.2 TGas send + 0.78 TGas execution).
/// Charged to the executor on top of the gas the action itself attaches.
pub const GAS_PER_ACTION: Gas = Gas::from_ggas(980);

/// Runtime fee to create one action receipt (`action_receipt_creation_config`, send + execution).
pub const GAS_PER_RECEIPT: Gas = Gas::from_ggas(217);

/// Gas `execute` burns in its own body: reading and removing the request, and building the batch.
pub const GAS_FOR_EXECUTE_BODY: Gas = Gas::from_tgas(5);

/// Upper bound for a request's total gas, checked at schedule time (NEAR caps prepaid gas at
/// 300 TGas). The 20 TGas margin absorbs a future repricing of the fees above.
pub const MAX_EXECUTION_GAS: Gas = Gas::from_tgas(280);

#[derive(BorshStorageKey)]
#[near(serializers = [borsh])]
enum StorageKey {
    Guardians,
    Requests,
}

#[near(contract_state)]
#[derive(PanicOnDefault)]
pub struct Contract {
    /// The Sputnik DAO account. The only account allowed to schedule requests.
    dao_id: AccountId,
    /// The only account allowed to change the configuration.
    admin_id: AccountId,
    /// Accounts allowed to cancel pending requests.
    guardians: IterableSet<AccountId>,
    /// Delay between scheduling and the earliest possible execution, in nanoseconds.
    delay_ns: u64,
    /// Id assigned to the next scheduled request.
    next_request_id: u64,
    /// Pending requests by id. Executed and cancelled requests are removed.
    requests: IterableMap<u64, Request>,
}

#[near]
impl Contract {
    #[init]
    pub fn new(
        dao_id: AccountId,
        admin_id: AccountId,
        guardians: Vec<AccountId>,
        delay_ns: U64,
    ) -> Self {
        require!(delay_ns.0 <= MAX_DELAY_NS, "Delay exceeds the maximum");
        let mut contract = Self {
            dao_id,
            admin_id,
            guardians: IterableSet::new(StorageKey::Guardians),
            delay_ns: delay_ns.0,
            next_request_id: 0,
            requests: IterableMap::new(StorageKey::Requests),
        };
        for guardian in guardians {
            contract.guardians.insert(guardian);
        }
        contract
    }

    /// Schedules a new request and returns its id. Only callable by the DAO.
    /// The attached deposit must equal the sum of the action deposits;
    /// The optional predecessor must leave the queue before this one can execute.
    #[payable]
    pub fn schedule(
        &mut self,
        receiver_id: AccountId,
        actions: Vec<FunctionCall>,
        predecessor_id: Option<u64>,
    ) -> u64 {
        self.assert_dao();
        require!(
            !actions.is_empty(),
            "Request must contain at least one action"
        );
        self.assert_predecessor(predecessor_id);
        let request = Request {
            receiver_id,
            actions,
            funder_id: env::predecessor_account_id(),
            execute_after: U64(env::block_timestamp().saturating_add(self.delay_ns)),
            approve: None,
            predecessor_id,
        };
        require!(
            env::attached_deposit() == request.total_deposit(),
            "Attached deposit must equal the total deposit of all actions"
        );
        require!(
            request.total_gas() <= MAX_EXECUTION_GAS,
            "Total action gas exceeds the executable limit"
        );
        let execute_after = request.execute_after;
        let request_id = self.insert_request(request);
        events::schedule(request_id, execute_after, predecessor_id);
        request_id
    }

    /// Schedules a Sputnik proposal on `dao_id`: `add_proposal`, then a `VoteApprove`
    /// in a callback; the target DAO must grant this contract the matching
    /// AddProposal/VoteApprove permissions. `proposal_bond` must match the target
    /// DAO's bond and equal the attached deposit; it is attached to `add_proposal`.
    #[payable]
    pub fn schedule_proposal(
        &mut self,
        description: String,
        kind: serde_json::Value,
        dao_id: AccountId,
        proposal_bond: NearToken,
        add_proposal_gas: Gas,
        act_proposal_gas: Gas,
        predecessor_id: Option<u64>,
    ) -> u64 {
        self.assert_dao();
        self.assert_predecessor(predecessor_id);
        require!(
            env::attached_deposit() == proposal_bond,
            "Attached deposit must equal the proposal bond"
        );
        let approval = ProposalApproval {
            kind: kind.to_string(),
            act_proposal_gas,
        };
        let args = serde_json::json!({
            "proposal": { "description": description, "kind": kind }
        });
        let request = Request {
            receiver_id: dao_id.clone(),
            actions: vec![FunctionCall {
                method_name: "add_proposal".to_string(),
                args: Base64VecU8::from(
                    serde_json::to_vec(&args).expect("Failed to serialize add_proposal args"),
                ),
                deposit: proposal_bond,
                gas: add_proposal_gas,
            }],
            funder_id: env::predecessor_account_id(),
            execute_after: U64(env::block_timestamp().saturating_add(self.delay_ns)),
            approve: Some(approval),
            predecessor_id,
        };
        require!(
            request.total_gas() <= MAX_EXECUTION_GAS,
            "Total action gas exceeds the executable limit"
        );
        let execute_after = request.execute_after;
        let request_id = self.insert_request(request);
        events::schedule_proposal(request_id, &dao_id, execute_after, predecessor_id);
        request_id
    }

    /// Executes a request whose delay has passed. Callable by anyone.
    /// The actions run as one atomic batch.
    /// The predecessor (if any) must have left the queue first.
    pub fn execute(&mut self, request_id: u64) -> Promise {
        let request = self
            .requests
            .remove(&request_id)
            .expect("Request not found");
        require!(
            env::block_timestamp() >= request.execute_after.0,
            "The timelock delay has not passed yet"
        );
        if let Some(predecessor_id) = request.predecessor_id {
            require!(
                !self.requests.contains_key(&predecessor_id),
                "Predecessor request is still pending"
            );
        }
        events::execute(request_id, &request.receiver_id);
        let total_deposit = request.total_deposit();
        let mut promise = Promise::new(request.receiver_id.clone());
        for action in request.actions {
            promise = promise.function_call(
                action.method_name,
                action.args.into(),
                action.deposit,
                action.gas,
            );
        }
        match request.approve {
            Some(approval) => promise.then(
                Self::ext(env::current_account_id())
                    .with_static_gas(
                        GAS_FOR_ON_PROPOSAL_ADDED.saturating_add(approval.act_proposal_gas),
                    )
                    .on_proposal_added(
                        request_id,
                        request.receiver_id,
                        request.funder_id,
                        total_deposit,
                        approval.kind,
                        approval.act_proposal_gas,
                    ),
            ),
            None => promise,
        }
    }

    /// Callback of a proposal request's `add_proposal` call: on success, votes
    /// `VoteApprove` on the returned proposal id; on failure, refunds the bond to the funder.
    #[private]
    pub fn on_proposal_added(
        &mut self,
        request_id: u64,
        dao_id: AccountId,
        funder_id: AccountId,
        bond: NearToken,
        kind: String,
        act_proposal_gas: Gas,
        #[callback_result] result: Result<u64, PromiseError>,
    ) -> PromiseOrValue<()> {
        match result {
            Ok(proposal_id) => {
                events::approve_proposal(request_id, proposal_id);
                let kind: serde_json::Value =
                    serde_json::from_str(&kind).expect("Stored proposal kind is not valid JSON");
                let args = serde_json::json!({
                    "id": proposal_id,
                    "action": "VoteApprove",
                    "proposal": kind,
                });
                PromiseOrValue::Promise(Promise::new(dao_id).function_call(
                    "act_proposal".to_string(),
                    serde_json::to_vec(&args).expect("Failed to serialize act_proposal args"),
                    NearToken::from_yoctonear(0),
                    act_proposal_gas,
                ))
            }
            Err(_) => {
                events::proposal_failed(request_id, &funder_id, bond);
                if bond.is_zero() {
                    PromiseOrValue::Value(())
                } else {
                    PromiseOrValue::Promise(Promise::new(funder_id).transfer(bond))
                }
            }
        }
    }

    /// Cancels a pending request and refunds the escrowed deposit to the funder.
    /// Callable only by a guardian.
    pub fn cancel(&mut self, request_id: u64) {
        let caller = env::predecessor_account_id();
        require!(
            self.guardians.contains(&caller),
            "Only a guardian can cancel requests"
        );
        let request = self
            .requests
            .remove(&request_id)
            .expect("Request not found");
        events::cancel(request_id, &request.receiver_id, &caller);
        let total_deposit = request.total_deposit();
        if !total_deposit.is_zero() {
            Promise::new(request.funder_id).transfer(total_deposit);
        }
    }

    pub fn get_dao(&self) -> &AccountId {
        &self.dao_id
    }

    pub fn get_admin(&self) -> &AccountId {
        &self.admin_id
    }

    pub fn get_guardians(&self) -> Vec<&AccountId> {
        self.guardians.iter().collect()
    }

    pub fn get_delay(&self) -> U64 {
        U64(self.delay_ns)
    }

    /// Total number of requests ever scheduled. Also the id of the next request.
    pub fn get_next_request_id(&self) -> u64 {
        self.next_request_id
    }

    pub fn get_num_requests(&self) -> u32 {
        self.requests.len()
    }

    pub fn get_request(&self, request_id: u64) -> Option<RequestOutput> {
        self.requests
            .get(&request_id)
            .map(|request| RequestOutput::new(request_id, request.clone()))
    }

    /// Returns pending requests. The order is arbitrary (not sorted by id).
    pub fn get_requests(&self, from_index: Option<u32>, limit: Option<u32>) -> Vec<RequestOutput> {
        let from_index =
            usize::try_from(from_index.unwrap_or(0)).expect("from_index exceeds usize");
        let limit = usize::try_from(limit.unwrap_or(u32::MAX)).expect("limit exceeds usize");
        self.requests
            .iter()
            .skip(from_index)
            .take(limit)
            .map(|(request_id, request)| RequestOutput::new(*request_id, request.clone()))
            .collect()
    }
}

impl Contract {
    /// Panics unless the caller is the DAO.
    fn assert_dao(&self) {
        require!(
            env::predecessor_account_id() == self.dao_id,
            "Only the DAO can schedule requests"
        );
    }

    /// Panics unless the predecessor (when given) is a pending request.
    fn assert_predecessor(&self, predecessor_id: Option<u64>) {
        if let Some(predecessor_id) = predecessor_id {
            require!(
                self.requests.contains_key(&predecessor_id),
                "Predecessor request not found"
            );
        }
    }

    /// Assigns the next id to the request, stores it, and returns the id.
    fn insert_request(&mut self, request: Request) -> u64 {
        let request_id = self.next_request_id;
        self.next_request_id += 1;
        self.requests.insert(request_id, request);
        request_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use near_sdk::test_utils::VMContextBuilder;
    use near_sdk::testing_env;

    const DELAY_NS: u64 = 24 * 60 * 60 * 1_000_000_000; // 1 day
    const START_TS: u64 = 1_000_000_000_000_000_000;

    fn dao() -> AccountId {
        "dao.near".parse().unwrap()
    }

    fn timelock() -> AccountId {
        "timelock.near".parse().unwrap()
    }

    fn admin() -> AccountId {
        "admin.near".parse().unwrap()
    }

    fn guardian() -> AccountId {
        "guardian.near".parse().unwrap()
    }

    fn context(predecessor: AccountId) -> VMContextBuilder {
        let mut builder = VMContextBuilder::new();
        builder
            .current_account_id(timelock())
            .predecessor_account_id(predecessor)
            .block_timestamp(START_TS);
        builder
    }

    fn new_contract() -> Contract {
        Contract::new(dao(), admin(), vec![guardian()], U64(DELAY_NS))
    }

    fn call_action(deposit: u128) -> FunctionCall {
        FunctionCall {
            method_name: "do_something".to_string(),
            args: Base64VecU8(b"{}".to_vec()),
            deposit: NearToken::from_yoctonear(deposit),
            gas: Gas::from_tgas(10),
        }
    }

    fn schedule_one(contract: &mut Contract, deposit: u128) -> u64 {
        schedule_after(contract, deposit, None)
    }

    fn schedule_after(contract: &mut Contract, deposit: u128, predecessor_id: Option<u64>) -> u64 {
        testing_env!(
            context(dao())
                .attached_deposit(NearToken::from_yoctonear(deposit))
                .build()
        );
        contract.schedule(
            "target.near".parse().unwrap(),
            vec![call_action(deposit)],
            predecessor_id,
        )
    }

    fn policy_kind() -> serde_json::Value {
        serde_json::json!({ "ChangePolicy": { "policy": { "proposal_bond": "1" } } })
    }

    fn schedule_proposal_one(contract: &mut Contract, bond: u128) -> u64 {
        testing_env!(
            context(dao())
                .attached_deposit(NearToken::from_yoctonear(bond))
                .build()
        );
        contract.schedule_proposal(
            "change the policy".to_string(),
            policy_kind(),
            dao(),
            NearToken::from_yoctonear(bond),
            Gas::from_tgas(30),
            Gas::from_tgas(100),
            None,
        )
    }

    // --- init & views ---

    #[test]
    fn test_init_and_views() {
        testing_env!(context(dao()).build());
        let contract = new_contract();
        assert_eq!(contract.get_dao(), &dao());
        assert_eq!(contract.get_admin(), &admin());
        assert_eq!(contract.get_guardians(), vec![&guardian()]);
        assert_eq!(contract.get_delay(), U64(DELAY_NS));
        assert_eq!(contract.get_num_requests(), 0);
        assert_eq!(contract.get_next_request_id(), 0);
    }

    // --- schedule ---

    #[test]
    fn test_schedule_by_dao() {
        testing_env!(context(dao()).build());
        let mut contract = new_contract();
        let request_id = schedule_one(&mut contract, 5);
        assert_eq!(request_id, 0);
        assert_eq!(contract.get_num_requests(), 1);
        assert_eq!(contract.get_next_request_id(), 1);
        let request = contract.get_request(request_id).unwrap();
        assert_eq!(
            request.receiver_id,
            "target.near".parse::<AccountId>().unwrap()
        );
        assert_eq!(request.funder_id, dao());
        assert_eq!(request.execute_after, U64(START_TS + DELAY_NS));
        assert_eq!(contract.get_requests(None, None).len(), 1);
    }

    #[test]
    #[should_panic(expected = "Only the DAO can schedule")]
    fn test_schedule_by_other_fails() {
        testing_env!(context(dao()).build());
        let mut contract = new_contract();
        testing_env!(context(guardian()).build());
        contract.schedule("target.near".parse().unwrap(), vec![call_action(0)], None);
    }

    #[test]
    #[should_panic(expected = "Attached deposit must equal")]
    fn test_schedule_wrong_deposit_fails() {
        testing_env!(context(dao()).build());
        let mut contract = new_contract();
        testing_env!(
            context(dao())
                .attached_deposit(NearToken::from_yoctonear(1))
                .build()
        );
        contract.schedule("target.near".parse().unwrap(), vec![call_action(5)], None);
    }

    #[test]
    #[should_panic(expected = "at least one action")]
    fn test_schedule_empty_fails() {
        testing_env!(context(dao()).build());
        let mut contract = new_contract();
        contract.schedule("target.near".parse().unwrap(), vec![], None);
    }

    #[test]
    #[should_panic(expected = "Total action gas exceeds")]
    fn test_schedule_excessive_gas_fails() {
        testing_env!(context(dao()).build());
        let mut contract = new_contract();
        let action = FunctionCall {
            method_name: "do_something".to_string(),
            args: Base64VecU8(b"{}".to_vec()),
            deposit: NearToken::from_yoctonear(0),
            gas: MAX_EXECUTION_GAS.saturating_sub(Gas::from_tgas(6)),
        };
        contract.schedule("target.near".parse().unwrap(), vec![action], None);
    }

    // --- schedule_proposal ---

    #[test]
    fn test_schedule_proposal_by_dao() {
        testing_env!(context(dao()).build());
        let mut contract = new_contract();
        let request_id = schedule_proposal_one(&mut contract, 7);
        assert_eq!(request_id, 0);
        assert_eq!(contract.get_next_request_id(), 1);

        let request = contract.get_request(request_id).unwrap();
        // The request targets the DAO with a single add_proposal call carrying the bond.
        assert_eq!(request.receiver_id, dao());
        assert_eq!(request.funder_id, dao());
        assert_eq!(request.execute_after, U64(START_TS + DELAY_NS));
        assert_eq!(request.actions.len(), 1);
        assert_eq!(request.actions[0].method_name, "add_proposal");
        assert_eq!(request.actions[0].deposit, NearToken::from_yoctonear(7));
        assert_eq!(request.actions[0].gas, Gas::from_tgas(30));
        let args: serde_json::Value = serde_json::from_slice(&request.actions[0].args.0).unwrap();
        assert_eq!(args["proposal"]["description"], "change the policy");
        assert_eq!(args["proposal"]["kind"], policy_kind());

        let approval = request.approve.unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&approval.kind).unwrap(),
            policy_kind()
        );
        assert_eq!(approval.act_proposal_gas, Gas::from_tgas(100));
    }

    #[test]
    fn test_schedule_proposal_for_other_dao() {
        testing_env!(context(dao()).build());
        let mut contract = new_contract();
        let other_dao: AccountId = "other-dao.near".parse().unwrap();
        let request_id = contract.schedule_proposal(
            "change the signers".to_string(),
            policy_kind(),
            other_dao.clone(),
            NearToken::from_yoctonear(0),
            Gas::from_tgas(30),
            Gas::from_tgas(100),
            None,
        );
        // The add_proposal call targets the given DAO, not the fronted one.
        let request = contract.get_request(request_id).unwrap();
        assert_eq!(request.receiver_id, other_dao);
        assert_eq!(request.actions[0].method_name, "add_proposal");
        assert!(request.approve.is_some());
    }

    #[test]
    #[should_panic(expected = "Only the DAO can schedule")]
    fn test_schedule_proposal_by_other_fails() {
        testing_env!(context(dao()).build());
        let mut contract = new_contract();
        testing_env!(context(guardian()).build());
        contract.schedule_proposal(
            "change the policy".to_string(),
            policy_kind(),
            dao(),
            NearToken::from_yoctonear(0),
            Gas::from_tgas(30),
            Gas::from_tgas(100),
            None,
        );
    }

    #[test]
    #[should_panic(expected = "Attached deposit must equal the proposal bond")]
    fn test_schedule_proposal_wrong_deposit_fails() {
        testing_env!(context(dao()).build());
        let mut contract = new_contract();
        testing_env!(
            context(dao())
                .attached_deposit(NearToken::from_yoctonear(1))
                .build()
        );
        contract.schedule_proposal(
            "change the policy".to_string(),
            policy_kind(),
            dao(),
            NearToken::from_yoctonear(7),
            Gas::from_tgas(30),
            Gas::from_tgas(100),
            None,
        );
    }

    // --- execute ---

    #[test]
    #[should_panic(expected = "delay has not passed")]
    fn test_execute_too_early_fails() {
        testing_env!(context(dao()).build());
        let mut contract = new_contract();
        let request_id = schedule_one(&mut contract, 0);
        testing_env!(
            context(dao())
                .block_timestamp(START_TS + DELAY_NS - 1)
                .build()
        );
        contract.execute(request_id);
    }

    #[test]
    fn test_execute_after_delay() {
        testing_env!(context(dao()).build());
        let mut contract = new_contract();
        let request_id = schedule_one(&mut contract, 5);
        // Anyone can execute after the delay.
        testing_env!(
            context("anyone.near".parse().unwrap())
                .block_timestamp(START_TS + DELAY_NS)
                .build()
        );
        contract.execute(request_id);
        assert_eq!(contract.get_num_requests(), 0);
        assert!(contract.get_request(request_id).is_none());
    }

    #[test]
    #[should_panic(expected = "Request not found")]
    fn test_execute_twice_fails() {
        testing_env!(context(dao()).build());
        let mut contract = new_contract();
        let request_id = schedule_one(&mut contract, 0);
        testing_env!(context(dao()).block_timestamp(START_TS + DELAY_NS).build());
        contract.execute(request_id);
        contract.execute(request_id);
    }

    #[test]
    #[should_panic(expected = "delay has not passed")]
    fn test_execute_proposal_too_early_fails() {
        testing_env!(context(dao()).build());
        let mut contract = new_contract();
        let request_id = schedule_proposal_one(&mut contract, 7);
        testing_env!(
            context(dao())
                .block_timestamp(START_TS + DELAY_NS - 1)
                .build()
        );
        contract.execute(request_id);
    }

    #[test]
    fn test_execute_proposal_after_delay() {
        testing_env!(context(dao()).build());
        let mut contract = new_contract();
        let request_id = schedule_proposal_one(&mut contract, 7);
        testing_env!(
            context("anyone.near".parse::<AccountId>().unwrap())
                .block_timestamp(START_TS + DELAY_NS)
                .build()
        );
        contract.execute(request_id);
        assert_eq!(contract.get_num_requests(), 0);
    }

    // --- predecessor ---

    #[test]
    fn test_schedule_with_predecessor() {
        testing_env!(context(dao()).build());
        let mut contract = new_contract();
        let first_id = schedule_one(&mut contract, 0);
        let second_id = schedule_after(&mut contract, 0, Some(first_id));
        assert_eq!(contract.get_request(first_id).unwrap().predecessor_id, None);
        assert_eq!(
            contract.get_request(second_id).unwrap().predecessor_id,
            Some(first_id)
        );
    }

    #[test]
    #[should_panic(expected = "Predecessor request not found")]
    fn test_schedule_unknown_predecessor_fails() {
        testing_env!(context(dao()).build());
        let mut contract = new_contract();
        schedule_after(&mut contract, 0, Some(42));
    }

    #[test]
    #[should_panic(expected = "Predecessor request not found")]
    fn test_schedule_executed_predecessor_fails() {
        testing_env!(context(dao()).build());
        let mut contract = new_contract();
        let first_id = schedule_one(&mut contract, 0);
        testing_env!(context(dao()).block_timestamp(START_TS + DELAY_NS).build());
        contract.execute(first_id);
        schedule_after(&mut contract, 0, Some(first_id));
    }

    #[test]
    fn test_schedule_with_proposal_predecessor() {
        testing_env!(context(dao()).build());
        let mut contract = new_contract();
        let proposal_id = schedule_proposal_one(&mut contract, 7);
        let request_id = schedule_after(&mut contract, 0, Some(proposal_id));
        assert_eq!(
            contract.get_request(request_id).unwrap().predecessor_id,
            Some(proposal_id)
        );
    }

    #[test]
    #[should_panic(expected = "Predecessor request is still pending")]
    fn test_execute_blocked_by_pending_predecessor() {
        testing_env!(context(dao()).build());
        let mut contract = new_contract();
        let first_id = schedule_one(&mut contract, 0);
        let second_id = schedule_after(&mut contract, 0, Some(first_id));
        testing_env!(context(dao()).block_timestamp(START_TS + DELAY_NS).build());
        contract.execute(second_id);
    }

    #[test]
    fn test_execute_after_predecessor_executed() {
        testing_env!(context(dao()).build());
        let mut contract = new_contract();
        let first_id = schedule_one(&mut contract, 0);
        let second_id = schedule_after(&mut contract, 0, Some(first_id));
        testing_env!(context(dao()).block_timestamp(START_TS + DELAY_NS).build());
        contract.execute(first_id);
        contract.execute(second_id);
        assert_eq!(contract.get_num_requests(), 0);
    }

    #[test]
    fn test_execute_after_predecessor_cancelled() {
        testing_env!(context(dao()).build());
        let mut contract = new_contract();
        let first_id = schedule_one(&mut contract, 0);
        let second_id = schedule_after(&mut contract, 0, Some(first_id));
        testing_env!(context(guardian()).build());
        contract.cancel(first_id);
        testing_env!(context(dao()).block_timestamp(START_TS + DELAY_NS).build());
        contract.execute(second_id);
        assert_eq!(contract.get_num_requests(), 0);
    }

    #[test]
    fn test_schedule_proposal_with_predecessor() {
        testing_env!(context(dao()).build());
        let mut contract = new_contract();
        let first_id = schedule_one(&mut contract, 0);
        testing_env!(context(dao()).build());
        let proposal_id = contract.schedule_proposal(
            "change the policy".to_string(),
            policy_kind(),
            dao(),
            NearToken::from_yoctonear(0),
            Gas::from_tgas(30),
            Gas::from_tgas(100),
            Some(first_id),
        );
        assert_eq!(
            contract.get_request(proposal_id).unwrap().predecessor_id,
            Some(first_id)
        );
    }

    // --- cancel ---

    #[test]
    fn test_cancel_by_guardian() {
        testing_env!(context(dao()).build());
        let mut contract = new_contract();
        let request_id = schedule_one(&mut contract, 5);
        testing_env!(context(guardian()).build());
        contract.cancel(request_id);
        assert_eq!(contract.get_num_requests(), 0);
    }

    #[test]
    #[should_panic(expected = "Only a guardian can cancel")]
    fn test_cancel_by_other_fails() {
        testing_env!(context(dao()).build());
        let mut contract = new_contract();
        let request_id = schedule_one(&mut contract, 0);
        testing_env!(context("anyone.near".parse().unwrap()).build());
        contract.cancel(request_id);
    }

    #[test]
    fn test_cancel_proposal_by_guardian() {
        testing_env!(context(dao()).build());
        let mut contract = new_contract();
        let request_id = schedule_proposal_one(&mut contract, 7);
        testing_env!(context(guardian()).build());
        contract.cancel(request_id);
        assert_eq!(contract.get_num_requests(), 0);
    }

    // --- setters ---

    #[test]
    fn test_set_dao_by_admin() {
        testing_env!(context(dao()).build());
        let mut contract = new_contract();
        testing_env!(context(admin()).build());
        let new_dao: AccountId = "dao2.near".parse().unwrap();
        contract.set_dao(new_dao.clone());
        assert_eq!(contract.get_dao(), &new_dao);
    }

    #[test]
    fn test_set_admin_by_admin() {
        testing_env!(context(dao()).build());
        let mut contract = new_contract();
        testing_env!(context(admin()).build());
        let new_admin: AccountId = "admin2.near".parse().unwrap();
        contract.set_admin(new_admin.clone());
        assert_eq!(contract.get_admin(), &new_admin);
    }

    #[test]
    #[should_panic(expected = "Only the admin")]
    fn test_set_admin_by_old_admin_fails() {
        testing_env!(context(dao()).build());
        let mut contract = new_contract();
        testing_env!(context(admin()).build());
        contract.set_admin("admin2.near".parse().unwrap());
        contract.set_admin(admin());
    }

    #[test]
    fn test_set_guardians_by_admin() {
        testing_env!(context(dao()).build());
        let mut contract = new_contract();
        testing_env!(context(admin()).build());
        let g2: AccountId = "guardian2.near".parse().unwrap();
        let g3: AccountId = "guardian3.near".parse().unwrap();
        contract.set_guardians(vec![g2.clone(), g3.clone()]);
        let guardians = contract.get_guardians();
        assert_eq!(guardians.len(), 2);
        assert!(guardians.contains(&&g2));
        assert!(guardians.contains(&&g3));
        assert!(!guardians.contains(&&guardian()));
    }

    #[test]
    fn test_set_delay_by_admin() {
        testing_env!(context(dao()).build());
        let mut contract = new_contract();
        testing_env!(context(admin()).build());
        contract.set_delay(U64(2 * DELAY_NS));
        assert_eq!(contract.get_delay(), U64(2 * DELAY_NS));
    }

    #[test]
    #[should_panic(expected = "Only the admin")]
    fn test_set_delay_by_dao_fails() {
        testing_env!(context(dao()).build());
        let mut contract = new_contract();
        contract.set_delay(U64(2 * DELAY_NS));
    }

    #[test]
    #[should_panic(expected = "Only the admin")]
    fn test_set_dao_by_self_fails() {
        testing_env!(context(dao()).build());
        let mut contract = new_contract();
        testing_env!(context(timelock()).build());
        contract.set_dao("dao2.near".parse().unwrap());
    }

    #[test]
    fn test_setters_by_self_when_self_admin() {
        // A timelock whose admin is itself is configured through its own
        // scheduled requests, like before the admin role existed.
        testing_env!(context(dao()).build());
        let mut contract = Contract::new(dao(), timelock(), vec![guardian()], U64(DELAY_NS));
        testing_env!(context(timelock()).build());
        contract.set_delay(U64(2 * DELAY_NS));
        assert_eq!(contract.get_delay(), U64(2 * DELAY_NS));
    }

    #[test]
    #[should_panic(expected = "Delay exceeds the maximum")]
    fn test_set_delay_too_large_fails() {
        testing_env!(context(dao()).build());
        let mut contract = new_contract();
        testing_env!(context(admin()).build());
        contract.set_delay(U64(MAX_DELAY_NS + 1));
    }
}
