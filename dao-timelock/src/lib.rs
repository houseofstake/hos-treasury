use near_sdk::json_types::{Base64VecU8, U64};
use near_sdk::store::{IterableMap, IterableSet};
use near_sdk::{
    AccountId, BorshStorageKey, Gas, NearToken, PanicOnDefault, Promise, PromiseError,
    PromiseOrValue, env, near, require,
};

mod events;
mod owner;

/// Upper bound for the execution delay: 90 days in nanoseconds.
/// Protects against a typo in a scheduled `set_delay` bricking the DAO forever.
pub const MAX_DELAY_NS: u64 = 90 * 24 * 60 * 60 * 1_000_000_000;

/// Gas reserved for the `on_proposal_added` callback itself, on top of the gas
/// it attaches to `act_proposal`.
pub const GAS_FOR_ON_PROPOSAL_ADDED: Gas = Gas::from_tgas(10);

#[derive(BorshStorageKey)]
#[near(serializers = [borsh])]
enum StorageKey {
    Guardians,
    Requests,
}

/// A single function call within a request. Mirrors the shape of a Sputnik
/// `ActionCall`: `args` are base64-encoded, `deposit` is attached to the call.
#[near(serializers = [borsh, json])]
#[derive(Clone)]
pub struct FunctionCall {
    pub method_name: String,
    pub args: Base64VecU8,
    pub deposit: NearToken,
    pub gas: Gas,
}

/// The follow-up vote of a proposal request: after the `add_proposal` action
/// resolves, the timelock approves the returned proposal id with this kind.
#[near(serializers = [borsh, json])]
#[derive(Clone)]
pub struct ProposalApproval {
    /// Raw JSON of the proposal kind, re-submitted in `act_proposal` so the DAO
    /// can verify the vote targets the intended proposal.
    pub kind: String,
    /// Gas attached to the `act_proposal` call made from the callback.
    pub act_proposal_gas: Gas,
}

/// A scheduled request: a batch of function calls to one receiver.
#[near(serializers = [borsh, json])]
#[derive(Clone)]
pub struct Request {
    /// Account the function calls will be sent to.
    pub receiver_id: AccountId,
    /// Function calls executed as a single atomic batch on the receiver.
    pub actions: Vec<FunctionCall>,
    /// Account that escrowed the deposits (the DAO at schedule time); refunded on cancel.
    pub funder_id: AccountId,
    /// Block timestamp (ns) after which the request can be executed.
    pub execute_after: U64,
    /// `Some` marks a proposal request: `actions` holds the single `add_proposal`
    /// call, followed by a `VoteApprove` on the proposal id it returns.
    pub approve: Option<ProposalApproval>,
}

impl Request {
    fn total_deposit(&self) -> NearToken {
        self.actions
            .iter()
            .fold(NearToken::from_yoctonear(0), |acc, action| {
                acc.saturating_add(action.deposit)
            })
    }
}

/// A pending request together with its id, as returned by view methods.
#[near(serializers = [json])]
pub struct RequestOutput {
    pub request_id: u64,
    pub receiver_id: AccountId,
    pub actions: Vec<FunctionCall>,
    pub funder_id: AccountId,
    pub execute_after: U64,
    pub approve: Option<ProposalApproval>,
}

impl RequestOutput {
    fn new(request_id: u64, request: Request) -> Self {
        Self {
            request_id,
            receiver_id: request.receiver_id,
            actions: request.actions,
            funder_id: request.funder_id,
            execute_after: request.execute_after,
            approve: request.approve,
        }
    }
}

#[near(contract_state)]
#[derive(PanicOnDefault)]
pub struct Contract {
    /// The Sputnik DAO account. The only account allowed to schedule requests.
    dao_id: AccountId,
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
    pub fn new(dao_id: AccountId, guardians: Vec<AccountId>, delay_ns: U64) -> Self {
        require!(delay_ns.0 <= MAX_DELAY_NS, "Delay exceeds the maximum");
        let mut contract = Self {
            dao_id,
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
    ///
    /// The attached deposit must equal the sum of the action deposits; it is
    /// escrowed and attached on execution or refunded to the funder on cancellation.
    #[payable]
    pub fn schedule(&mut self, receiver_id: AccountId, actions: Vec<FunctionCall>) -> u64 {
        require!(
            env::predecessor_account_id() == self.dao_id,
            "Only the DAO can schedule requests"
        );
        require!(
            !actions.is_empty(),
            "Request must contain at least one action"
        );
        let request = Request {
            receiver_id,
            actions,
            funder_id: env::predecessor_account_id(),
            execute_after: U64(env::block_timestamp().saturating_add(self.delay_ns)),
            approve: None,
        };
        require!(
            env::attached_deposit() == request.total_deposit(),
            "Attached deposit must equal the total deposit of all actions"
        );
        self.insert_request(events::schedule, request)
    }

    /// Schedules a Sputnik proposal on the DAO behind this timelock and returns the
    /// request id. Only callable by the DAO.
    ///
    /// Executing the request calls `add_proposal`, then votes `VoteApprove` in a
    /// callback on the returned id (ids cannot be predicted at schedule time).
    ///
    /// The attached deposit is the proposal bond, escrowed like a generic request and
    /// refunded if the request is cancelled or `add_proposal` fails.
    ///
    /// Approving a proposal executes it, so `act_proposal_gas` must also cover the
    /// proposal's own action.
    #[payable]
    pub fn schedule_proposal(
        &mut self,
        description: String,
        kind: serde_json::Value,
        add_proposal_gas: Gas,
        act_proposal_gas: Gas,
    ) -> u64 {
        require!(
            env::predecessor_account_id() == self.dao_id,
            "Only the DAO can schedule requests"
        );
        let approval = ProposalApproval {
            kind: kind.to_string(),
            act_proposal_gas,
        };
        let args = serde_json::json!({
            "proposal": { "description": description, "kind": kind }
        });
        let request = Request {
            receiver_id: self.dao_id.clone(),
            actions: vec![FunctionCall {
                method_name: "add_proposal".to_string(),
                args: Base64VecU8::from(
                    serde_json::to_vec(&args).expect("Failed to serialize add_proposal args"),
                ),
                deposit: env::attached_deposit(),
                gas: add_proposal_gas,
            }],
            funder_id: env::predecessor_account_id(),
            execute_after: U64(env::block_timestamp().saturating_add(self.delay_ns)),
            approve: Some(approval),
        };
        self.insert_request(events::schedule_proposal, request)
    }

    /// Executes a request whose delay has passed. Callable by anyone: the DAO already
    /// approved it at schedule time and no guardian cancelled it during the delay.
    ///
    /// The request is removed up front so it can never run twice. The actions run as
    /// one atomic batch; if any fails, the deposits return to this contract's balance.
    pub fn execute(&mut self, request_id: u64) -> Promise {
        let request = self
            .requests
            .remove(&request_id)
            .expect("Request not found");
        require!(
            env::block_timestamp() >= request.execute_after.0,
            "The timelock delay has not passed yet"
        );
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
    /// Callable by any guardian or by the DAO.
    pub fn cancel(&mut self, request_id: u64) {
        let caller = env::predecessor_account_id();
        require!(
            caller == self.dao_id || self.guardians.contains(&caller),
            "Only a guardian or the DAO can cancel requests"
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
        let from_index = from_index.unwrap_or(0) as usize;
        let limit = limit.unwrap_or(u32::MAX) as usize;
        self.requests
            .iter()
            .skip(from_index)
            .take(limit)
            .map(|(request_id, request)| RequestOutput::new(*request_id, request.clone()))
            .collect()
    }
}

impl Contract {
    /// Assigns the next id to the request, emits the given schedule event and stores it.
    fn insert_request(
        &mut self,
        emit: fn(u64, &AccountId, U64, NearToken),
        request: Request,
    ) -> u64 {
        let request_id = self.next_request_id;
        self.next_request_id += 1;
        emit(
            request_id,
            &request.receiver_id,
            request.execute_after,
            request.total_deposit(),
        );
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
        Contract::new(dao(), vec![guardian()], U64(DELAY_NS))
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
        testing_env!(
            context(dao())
                .attached_deposit(NearToken::from_yoctonear(deposit))
                .build()
        );
        contract.schedule("target.near".parse().unwrap(), vec![call_action(deposit)])
    }

    #[test]
    fn test_init_and_views() {
        testing_env!(context(dao()).build());
        let contract = new_contract();
        assert_eq!(contract.get_dao(), &dao());
        assert_eq!(contract.get_guardians(), vec![&guardian()]);
        assert_eq!(contract.get_delay(), U64(DELAY_NS));
        assert_eq!(contract.get_num_requests(), 0);
        assert_eq!(contract.get_next_request_id(), 0);
    }

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
        contract.schedule("target.near".parse().unwrap(), vec![call_action(0)]);
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
        contract.schedule("target.near".parse().unwrap(), vec![call_action(5)]);
    }

    #[test]
    #[should_panic(expected = "at least one action")]
    fn test_schedule_empty_fails() {
        testing_env!(context(dao()).build());
        let mut contract = new_contract();
        contract.schedule("target.near".parse().unwrap(), vec![]);
    }

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
    fn test_cancel_by_guardian() {
        testing_env!(context(dao()).build());
        let mut contract = new_contract();
        let request_id = schedule_one(&mut contract, 5);
        testing_env!(context(guardian()).build());
        contract.cancel(request_id);
        assert_eq!(contract.get_num_requests(), 0);
    }

    #[test]
    fn test_cancel_by_dao() {
        testing_env!(context(dao()).build());
        let mut contract = new_contract();
        let request_id = schedule_one(&mut contract, 0);
        testing_env!(context(dao()).build());
        contract.cancel(request_id);
        assert_eq!(contract.get_num_requests(), 0);
    }

    #[test]
    #[should_panic(expected = "Only a guardian or the DAO can cancel")]
    fn test_cancel_by_other_fails() {
        testing_env!(context(dao()).build());
        let mut contract = new_contract();
        let request_id = schedule_one(&mut contract, 0);
        testing_env!(context("anyone.near".parse().unwrap()).build());
        contract.cancel(request_id);
    }

    #[test]
    fn test_set_dao_by_self() {
        testing_env!(context(dao()).build());
        let mut contract = new_contract();
        testing_env!(context(timelock()).build());
        let new_dao: AccountId = "dao2.near".parse().unwrap();
        contract.set_dao(new_dao.clone());
        assert_eq!(contract.get_dao(), &new_dao);
    }

    #[test]
    #[should_panic(expected = "Only callable by the timelock itself")]
    fn test_set_dao_by_dao_fails() {
        testing_env!(context(dao()).build());
        let mut contract = new_contract();
        contract.set_dao("dao2.near".parse().unwrap());
    }

    #[test]
    fn test_set_guardians_by_self() {
        testing_env!(context(dao()).build());
        let mut contract = new_contract();
        testing_env!(context(timelock()).build());
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
    #[should_panic(expected = "Only callable by the timelock itself")]
    fn test_set_guardians_by_guardian_fails() {
        testing_env!(context(dao()).build());
        let mut contract = new_contract();
        testing_env!(context(guardian()).build());
        contract.set_guardians(vec![]);
    }

    #[test]
    fn test_set_delay_by_self() {
        testing_env!(context(dao()).build());
        let mut contract = new_contract();
        testing_env!(context(timelock()).build());
        contract.set_delay(U64(2 * DELAY_NS));
        assert_eq!(contract.get_delay(), U64(2 * DELAY_NS));
    }

    #[test]
    #[should_panic(expected = "Delay exceeds the maximum")]
    fn test_set_delay_too_large_fails() {
        testing_env!(context(dao()).build());
        let mut contract = new_contract();
        testing_env!(context(timelock()).build());
        contract.set_delay(U64(MAX_DELAY_NS + 1));
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
            Gas::from_tgas(30),
            Gas::from_tgas(100),
        )
    }

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
    #[should_panic(expected = "Only the DAO can schedule")]
    fn test_schedule_proposal_by_other_fails() {
        testing_env!(context(dao()).build());
        let mut contract = new_contract();
        testing_env!(context(guardian()).build());
        contract.schedule_proposal(
            "change the policy".to_string(),
            policy_kind(),
            Gas::from_tgas(30),
            Gas::from_tgas(100),
        );
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
    fn test_cancel_proposal_by_guardian() {
        testing_env!(context(dao()).build());
        let mut contract = new_contract();
        let request_id = schedule_proposal_one(&mut contract, 7);
        testing_env!(context(guardian()).build());
        contract.cancel(request_id);
        assert_eq!(contract.get_num_requests(), 0);
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
}
