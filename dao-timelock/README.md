# dao-timelock

A timelock contract that sits between a [Sputnik DAO](https://github.com/near-daos/sputnik-dao-contract) and every action it takes. Instead of executing actions directly, the DAO schedules them here; they become executable only after a configured delay, and during that delay any guardian can cancel them.

## Features

- **Only the DAO can schedule.** A request is a batch of function calls (method, base64 args, deposit, gas) to a single receiver, executed atomically on that receiver.
- **Anyone can execute** a request once its delay has passed — it was already approved by the DAO and survived the guardian review window. A request is removed from state before the promise is created, so it can never execute twice.
- **Optional ordering between requests.** A request may name a pending request as its `predecessor_id`; it then becomes executable only once the predecessor has been executed or cancelled. NEAR processes receipts to the same receiver in dispatch order, so at a shared receiver the predecessor's batch runs before the dependent one (use this for e.g. upgrade-then-migrate). Caveats: the predecessor's *success* is not checked; ordering across different receivers is dispatch-order only; a proposal-request predecessor is only ordered up to its `add_proposal` dispatch (the approving vote and the proposal's own effects fire later, from callbacks); cancelling a predecessor unblocks its dependents, so guardians should review those too.
- **Any guardian can cancel** a pending request. The escrowed deposit is refunded to the funder.
- **Config changes are admin-only.** `set_dao`, `set_admin`, `set_guardians`, and `set_delay` are only callable by the configured admin. The admin is expected to be a timelock itself — the Policy Timelock in the HoS topology, or the contract's own account for a self-administered timelock — so config changes are scheduled as requests and wait out a delay, and guardians can cancel them like any other request.
- **Not upgradable by design.** There is no method to deploy new code or migrate state. To move to a new timelock, the DAO points its own workflows at a new contract; to move to a new DAO, the admin schedules `set_dao`.

## Deposit semantics

`schedule` is payable and the attached deposit must exactly equal the sum of the `deposit` fields of all actions; for `schedule_proposal` it must equal `proposal_bond`. The deposit is escrowed by the timelock:

- On **execute**, the deposits are attached to the outgoing function calls.
- On **cancel**, the full amount is refunded to the funder (the DAO account that scheduled the request).
- If **execution fails** on the receiver, the whole batch reverts and NEAR's protocol-level refund returns the deposits to the timelock contract's balance (not the DAO). Keep this in mind for large deposits. Exception: when a proposal request's `add_proposal` fails, the callback explicitly refunds the bond to the funder.

The timelock should hold a small NEAR balance to cover storage for pending requests.

## Typical flows

### DAO calls another contract

1. DAO proposal: `FunctionCall` to `timelock.near::schedule` with `{receiver_id, actions}` and the total deposit attached.
2. Proposal approved → request queued with `execute_after = now + delay`.
3. After the delay, anyone calls `timelock.near::execute {request_id}`.

### DAO changes Sputnik signers (its own or another DAO's policy)

`schedule_proposal` schedules an `add_proposal` on the target `dao_id` and, in a callback, approves whatever proposal id the DAO returns — so proposals landing on the target during the delay cannot break the flow by shifting the id sequence. The target can be the DAO the timelock fronts or any other DAO — e.g. the Policy Timelock in the HoS topology changes the signers of the Execution and Payment DAOs with a single request. The target DAO's policy must grant the timelock the matching `AddProposal`/`VoteApprove` permissions (e.g. `policy:*` for `ChangePolicy` proposals), and `proposal_bond` must equal the target DAO's proposal bond — it is escrowed at schedule time (the attached deposit must equal it) and attached to the inner `add_proposal`. When a DAO targets itself, its self-updates are timelocked because the DAO grants the timelock (not its members directly) the permission to make them.

### Admin updates a timelock's guardians

On the admin timelock: `schedule` with `receiver_id = <target timelock account>` and action `set_guardians {"guardians": [...]}`. After the delay, `execute` makes the admin timelock call the target, which passes the admin-only check. A self-administered timelock is the target of its own request.

## API

```rust
// Init (contract is PanicOnDefault; deploy + call new)
new(dao_id: AccountId, admin_id: AccountId, guardians: Vec<AccountId>, delay_ns: U64)

// State-changing
#[payable] schedule(receiver_id: AccountId, actions: Vec<FunctionCall>,
                    predecessor_id: Option<u64>) -> u64                         // DAO only
#[payable] schedule_proposal(description: String, kind: Value, dao_id: AccountId,
                             proposal_bond: NearToken, add_proposal_gas: Gas,
                             act_proposal_gas: Gas,
                             predecessor_id: Option<u64>) -> u64                 // DAO only
execute(request_id: u64) -> Promise                                            // anyone, after delay
cancel(request_id: u64)                                                        // guardian

// Admin-only (expected to be scheduled through the admin timelock)
set_dao(dao_id: AccountId)
set_admin(admin_id: AccountId)
set_guardians(guardians: Vec<AccountId>)
set_delay(delay_ns: U64)  // capped at 30 days

// Views
get_dao() -> AccountId
get_admin() -> AccountId
get_guardians() -> Vec<AccountId>
get_delay() -> U64
get_next_request_id() -> u64
get_num_requests() -> u32
get_request(request_id: u64) -> Option<RequestOutput>
get_requests(from_index: Option<u32>, limit: Option<u32>) -> Vec<RequestOutput>
```

`FunctionCall` JSON shape:

```json
{
  "method_name": "some_method",
  "args": "<base64 of the JSON args>",
  "deposit": "1000000000000000000000000",
  "gas": 30000000000000
}
```

Request lifecycle changes emit `EVENT_JSON` logs (standard `dao-timelock`, events `schedule`, `schedule_proposal`, `execute`, `approve_proposal`, `proposal_failed`, `cancel`).
