# dao-timelock

A timelock contract that sits between a [Sputnik DAO](https://github.com/near-daos/sputnik-dao-contract) and every action it takes. Instead of executing actions directly, the DAO schedules them here; they become executable only after a configured delay, and during that delay any guardian can cancel them.

## Design

- **Only the DAO can schedule.** A request is a batch of function calls (method, base64 args, deposit, gas) to a single receiver, executed atomically on that receiver.
- **Anyone can execute** a request once its delay has passed — it was already approved by the DAO and survived the guardian review window. A request is removed from state before the promise is created, so it can never execute twice.
- **Any guardian (or the DAO) can cancel** a pending request. The escrowed deposit is refunded to the funder.
- **Config changes go through the timelock too.** `set_dao`, `set_guardians`, and `set_delay` are only callable by the timelock account itself, so the DAO must schedule them as requests targeting the timelock and wait out the same delay. Guardians can cancel these like any other request — including an attempt to remove the guardians.
- **Not upgradable by design.** There is no method to deploy new code or migrate state. To move to a new timelock, the DAO points its own workflows at a new contract; to move to a new DAO, the current DAO schedules `set_dao`.

## Deposit semantics

`schedule` is payable and the attached deposit must exactly equal the sum of the `deposit` fields of all actions. The deposit is escrowed by the timelock:

- On **execute**, the deposits are attached to the outgoing function calls.
- On **cancel**, the full amount is refunded to the funder (the DAO account that scheduled the request).
- If **execution fails** on the receiver, the whole batch reverts and NEAR's protocol-level refund returns the deposits to the timelock contract's balance (not the DAO). Keep this in mind for large deposits.

The timelock should hold a small NEAR balance to cover storage for pending requests.

## Typical flows

### DAO calls another contract

1. DAO proposal: `FunctionCall` to `timelock.near::schedule` with `{receiver_id, actions}` and the total deposit attached.
2. Proposal approved → request queued with `execute_after = now + delay`.
3. After the delay, anyone calls `timelock.near::execute {request_id}`.

### DAO changes its own approvers (Sputnik policy)

Same as above with `receiver_id = <dao account>` and an action calling the DAO's policy-change method — the DAO's self-updates are timelocked because the DAO grants the timelock (not its members directly) the permission to make them.

### DAO updates the timelock's guardians

`schedule` with `receiver_id = <timelock account>` and action `set_guardians {"guardians": [...]}`. After the delay, `execute` makes the timelock call itself, which passes the self-only check.

## API

```rust
// Init (contract is PanicOnDefault; deploy + call new)
new(dao_id: AccountId, guardians: Vec<AccountId>, delay_ns: U64)

// State-changing
#[payable] schedule(receiver_id: AccountId, actions: Vec<FunctionCall>) -> u64  // DAO only
execute(request_id: u64) -> Promise                                            // anyone, after delay
cancel(request_id: u64)                                                        // guardian or DAO

// Self-only (must be scheduled through the timelock)
set_dao(dao_id: AccountId)
set_guardians(guardians: Vec<AccountId>)
set_delay(delay_ns: U64)  // capped at 366 days

// Views
get_dao() -> AccountId
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

All state changes emit `EVENT_JSON` logs (standard `dao-timelock`, events `schedule`, `execute`, `cancel`, `set_dao`, `set_guardians`, `set_delay`).
