# spending-account

A treasury contract that holds a large NEAR balance (and optionally NEP-141 token balances) and pays it out only to whitelisted accounts, each capped by a spending limit. Control is split between three roles, each held by an account that is expected to be a [Sputnik DAO](https://github.com/near-daos/sputnik-dao-contract) acting through its own [`dao-timelock`](../dao-timelock).

## Design

- **Three independent roles.** The **spender** can only transfer NEAR or whitelisted NEP-141 tokens to already-whitelisted accounts within their remaining allowance — nothing else. The **manager** manages the whitelist and the limits — but cannot transfer funds. The **admin** only assigns the roles (`set_spender`, `set_manager`, `set_admin`) and cannot touch the funds, the whitelist or the limits. Each role is checked separately, so they can be held by three different accounts or, as in the [HoS topology](../README.md#architecture), by fewer: there the spender and the manager are the same timelock and the admin is a different one. Either way every role's actions go through its DAO's timelock, where guardians can cancel them.
- **Limits don't expire.** Each whitelisted account's limit is its remaining allowance: transfers decrease it and there is no periodic reset. To grant a new allowance, the manager raises the account's limit (`increase_limit` or `set_limit`).
- **One whitelist, per-token entries.** Every entry is a (receiver, token) pair with its own limit in the token's smallest unit; `token_id: None` denotes native NEAR with the limit in yoctoNEAR. Being whitelisted for NEAR grants no token allowance and vice versa. The receiver must be registered with the token contract (NEP-145 storage deposit); this contract does not pay storage deposits. The 1 yoctoNEAR required by `ft_transfer` is paid from this contract's balance.
- **Failed transfers don't burn allowance.** The limit is charged before the transfer is sent; if the transfer fails (e.g. the receiver account doesn't exist, or is not registered with the token), the funds stay on this contract's balance and the `on_transfer` / `on_transfer_ft` callback restores the limit. The rollback is skipped if the record was removed in the meantime.
- **Not upgradable by design.** There is no method to deploy new code or migrate state. To move to a new setup, the admin re-points the roles or the DAOs deploy a fresh contract and move the funds via whitelisted transfers.

## API

```rust
// Init (contract is PanicOnDefault; deploy + call new)
new(admin_id: AccountId, manager_id: AccountId, spender_id: AccountId)

// Spender only
transfer(receiver_id: AccountId, amount: NearToken) -> Promise
transfer_ft(receiver_id: AccountId, token_id: AccountId, amount: U128) -> Promise

// Manager only. All methods take a non-empty batch and apply it atomically:
// any invalid element panics and reverts the whole call. token_id None =
// native NEAR (limits/amounts in yoctoNEAR), Some = a NEP-141 token
// (limits/amounts in its smallest unit).
add_to_whitelist(entries: Vec<WhitelistEntry>)
remove_from_whitelist(keys: Vec<WhitelistEntryKey>)      // {account_id, token_id}
set_limit(entries: Vec<WhitelistEntry>)
increase_limit(changes: Vec<LimitChange>)                // {account_id, token_id, amount}
decrease_limit(changes: Vec<LimitChange>)                // floors at zero

// Admin only
set_spender(spender_id: AccountId)
set_manager(manager_id: AccountId)
set_admin(admin_id: AccountId)

// Views (same token_id convention)
get_spender() -> AccountId
get_manager() -> AccountId
get_admin() -> AccountId
get_num_whitelisted() -> u32
get_whitelist_entry(account_id: AccountId, token_id: Option<AccountId>) -> Option<WhitelistEntry>
get_whitelist_entries(from_index: Option<u32>, limit: Option<u32>) -> Vec<WhitelistEntry>
```

`WhitelistEntry {account_id, token_id, limit}` carries the (remaining) `limit` in both arguments and view results. `NearToken` amounts and `U128` limits are JSON strings (yoctoNEAR for native NEAR, the smallest unit for tokens).

Transfers emit `EVENT_JSON` logs (standard `spending-account`, events `transfer`, `transfer_failed`, `ft_transfer`, `ft_transfer_failed`).
