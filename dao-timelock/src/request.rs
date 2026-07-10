use crate::*;

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

/// The follow-up vote of a proposal request: after the `add_proposal` action.
#[near(serializers = [borsh, json])]
#[derive(Clone)]
pub struct ProposalApproval {
    /// Raw JSON of the proposal kind.
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
    /// Account that escrowed the deposits; refunded on cancel.
    pub funder_id: AccountId,
    /// Block timestamp (ns) after which the request can be executed.
    pub execute_after: U64,
    /// When set, this request adds a DAO proposal; the timelock then casts a
    /// follow-up approving vote on the proposal id the DAO returns. `None` is plain.
    pub approve: Option<ProposalApproval>,
    /// When set, this request can only be executed after the referenced request
    /// is no longer pending (executed or cancelled).
    pub predecessor_id: Option<u64>,
}

impl Request {
    pub(crate) fn total_deposit(&self) -> NearToken {
        self.actions
            .iter()
            .fold(NearToken::from_yoctonear(0), |acc, action| {
                acc.saturating_add(action.deposit)
            })
    }

    /// Total gas the request attaches at execution time.
    pub(crate) fn total_gas(&self) -> Gas {
        let actions_gas = self.actions.iter().fold(Gas::from_gas(0), |acc, action| {
            acc.saturating_add(action.gas)
        });
        match &self.approve {
            Some(approval) => actions_gas
                .saturating_add(GAS_FOR_ON_PROPOSAL_ADDED)
                .saturating_add(approval.act_proposal_gas),
            None => actions_gas,
        }
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
    pub predecessor_id: Option<u64>,
}

impl RequestOutput {
    pub(crate) fn new(request_id: u64, request: Request) -> Self {
        Self {
            request_id,
            receiver_id: request.receiver_id,
            actions: request.actions,
            funder_id: request.funder_id,
            execute_after: request.execute_after,
            approve: request.approve,
            predecessor_id: request.predecessor_id,
        }
    }
}
