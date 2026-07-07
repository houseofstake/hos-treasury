# spending-account

A treasury contract that holds a large NEAR balance and pays it out only to whitelisted accounts, each capped by a yearly spending limit. Control is split between two accounts, each expected to be a [Sputnik DAO](https://github.com/near-daos/sputnik-dao-contract) acting through its own [`dao-timelock`](../dao-timelock).

## Design

- **Two separated roles.** The **spender** can only transfer NEAR to already-whitelisted accounts within their remaining yearly allowance — nothing else. The **admin** manages the whitelist, the yearly limits and the role assignments (`set_spender`, `set_admin`) — but cannot transfer funds. Compromising either role alone is not enough to drain the treasury quickly: the spender is bounded by the whitelist and its limits, and the admin's changes go through its DAO's timelock where guardians can cancel them.
- **Yearly limits over fixed periods.** Periods are 365 days long, counted from `first_period_start` (set at init, e.g. to a calendar year boundary; must not be in the future). Each whitelisted account's spent amount lazily resets at the start of every period. Changing a limit mid-period keeps the spent amount; lowering it below what was already spent just leaves zero available until the next period.
- **Failed transfers don't burn allowance.** The spent counter is charged before the transfer is sent; if the transfer fails (e.g. the receiver account doesn't exist), the protocol refunds the NEAR to this contract and the `on_transfer` callback rolls the counter back. The rollback is skipped if the record was removed or its period rolled over in the meantime.
- **Not upgradable by design.** There is no method to deploy new code or migrate state. To move to a new setup, the admin re-points the roles or the DAOs deploy a fresh contract and move the funds via whitelisted transfers.

## Caveats

- Removing and re-adding an account within the same period resets its spent amount, making its full limit available again. The admin role is trusted (and timelocked), so this is a feature, not a bug — but worth knowing.
- Funding the contract is a plain NEAR transfer to its account; no method call is needed.
- The contract must retain enough balance to cover its own storage; a transfer that would drop it below the protocol's storage requirement fails and is rolled back like any other failed transfer.

## Typical flow

1. Deploy and call `new(admin_id, spender_id, first_period_start?)` with the two timelock accounts.
2. The admin DAO schedules `add_to_whitelist {account_id, yearly_limit}` on its timelock; after the delay it executes against this contract.
3. The spender DAO schedules `transfer {receiver_id, amount}` on its timelock; after the delay the transfer executes, bounded by the receiver's remaining allowance.

## API

```rust
// Init (contract is PanicOnDefault; deploy + call new)
new(admin_id: AccountId, spender_id: AccountId, first_period_start: Option<U64>)

// Spender only
transfer(receiver_id: AccountId, amount: NearToken) -> Promise

// Admin only
add_to_whitelist(account_id: AccountId, yearly_limit: NearToken)
remove_from_whitelist(account_id: AccountId)
set_yearly_limit(account_id: AccountId, yearly_limit: NearToken)
set_spender(spender_id: AccountId)
set_admin(admin_id: AccountId)

// Views
get_spender() -> AccountId
get_admin() -> AccountId
is_whitelisted(account_id: AccountId) -> bool
get_num_whitelisted() -> u32
get_whitelist_entry(account_id: AccountId) -> Option<WhitelistEntry>
get_whitelist(from_index: Option<u32>, limit: Option<u32>) -> Vec<WhitelistEntry>
get_period_info() -> PeriodInfo
```

`WhitelistEntry` reports `spent` and `available` computed for the current period. `NearToken` amounts are JSON strings in yoctoNEAR.

All state changes emit `EVENT_JSON` logs (standard `spending-account`, events `transfer`, `transfer_failed`, `add_to_whitelist`, `remove_from_whitelist`, `set_yearly_limit`, `set_spender`, `set_admin`).
