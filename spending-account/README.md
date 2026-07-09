# spending-account

A treasury contract that holds a large NEAR balance (and optionally NEP-141 token balances) and pays it out only to whitelisted accounts, each capped by a spending limit. Control is split between two accounts, each expected to be a [Sputnik DAO](https://github.com/near-daos/sputnik-dao-contract) acting through its own [`dao-timelock`](../dao-timelock).

## Design

- **Two separated roles.** The **spender** can only transfer NEAR or whitelisted NEP-141 tokens to already-whitelisted accounts within their remaining allowance — nothing else. The **admin** manages the whitelist, the limits and the role assignments (`set_spender`, `set_admin`) — but cannot transfer funds. Compromising either role alone is not enough to drain the treasury quickly: the spender is bounded by the whitelist and its limits, and the admin's changes go through its DAO's timelock where guardians can cancel them.
- **Limits don't expire.** Each whitelisted account's limit is its remaining allowance: transfers decrease it and there is no periodic reset. To grant a new allowance, the admin raises the account's limit (`increase_limit` or `set_limit`).
- **One whitelist, per-token entries.** Every entry is a (receiver, token) pair with its own limit in the token's smallest unit; `token_id: None` denotes native NEAR with the limit in yoctoNEAR. Being whitelisted for NEAR grants no token allowance and vice versa. The receiver must be registered with the token contract (NEP-145 storage deposit); this contract does not pay storage deposits. The 1 yoctoNEAR required by `ft_transfer` is paid from this contract's balance.
- **Failed transfers don't burn allowance.** The limit is charged before the transfer is sent; if the transfer fails (e.g. the receiver account doesn't exist, or is not registered with the token), the funds stay on this contract's balance and the `on_transfer` / `on_transfer_ft` callback restores the limit. The rollback is skipped if the record was removed in the meantime.
- **Not upgradable by design.** There is no method to deploy new code or migrate state. To move to a new setup, the admin re-points the roles or the DAOs deploy a fresh contract and move the funds via whitelisted transfers.

## Caveats

- If a record is removed and re-added while a failed transfer's callback is still in flight, the refund is added on top of the fresh limit. The admin role is trusted (and timelocked) and can always correct the limit, so this is only worth knowing, not a practical risk.
- Funding the contract is a plain NEAR transfer to its account; no method call is needed. Token balances are funded with a plain `ft_transfer` to its account (register the contract with the token first).
- The contract must retain enough balance to cover its own storage; a transfer that would drop it below the protocol's storage requirement fails and is rolled back like any other failed transfer.

## Typical flow

1. Deploy and call `new(admin_id, spender_id)` with the two timelock accounts.
2. The admin DAO schedules `add_to_whitelist {account_id, token_id, limit}` on its timelock (`token_id: null` for native NEAR); after the delay it executes against this contract.
3. The spender DAO schedules `transfer {receiver_id, amount}` (or `transfer_ft {receiver_id, token_id, amount}`) on its timelock; after the delay the transfer executes, bounded by the receiver's remaining allowance.

## API

```rust
// Init (contract is PanicOnDefault; deploy + call new)
new(admin_id: AccountId, spender_id: AccountId)

// Spender only
transfer(receiver_id: AccountId, amount: NearToken) -> Promise
transfer_ft(receiver_id: AccountId, token_id: AccountId, amount: U128) -> Promise

// Admin only. token_id None = native NEAR (limits/amounts in yoctoNEAR),
// Some = a NEP-141 token (limits/amounts in its smallest unit).
add_to_whitelist(account_id: AccountId, token_id: Option<AccountId>, limit: U128)
remove_from_whitelist(account_id: AccountId, token_id: Option<AccountId>)
set_limit(account_id: AccountId, token_id: Option<AccountId>, limit: U128)
increase_limit(account_id: AccountId, token_id: Option<AccountId>, amount: U128)
decrease_limit(account_id: AccountId, token_id: Option<AccountId>, amount: U128)  // floors at zero
set_spender(spender_id: AccountId)
set_admin(admin_id: AccountId)

// Views (same token_id convention)
get_spender() -> AccountId
get_admin() -> AccountId
is_whitelisted(account_id: AccountId, token_id: Option<AccountId>) -> bool
get_num_whitelisted() -> u32
get_whitelist_entry(account_id: AccountId, token_id: Option<AccountId>) -> Option<WhitelistEntry>
get_whitelist(from_index: Option<u32>, limit: Option<u32>) -> Vec<WhitelistEntry>
```

`WhitelistEntry {account_id, token_id, limit}` reports the remaining `limit`. `NearToken` amounts and `U128` limits are JSON strings (yoctoNEAR for native NEAR, the smallest unit for tokens).

Transfers emit `EVENT_JSON` logs (standard `spending-account`, events `transfer`, `transfer_failed`, `ft_transfer`, `ft_transfer_failed`).
