mod setup;

use crate::setup::{TreasuryTestWorkspaceBuilder, assert_almost_eq, outcome_check};
use near_sdk::NearToken;
use near_workspaces::AccountId;
use serde_json::json;
use std::str::FromStr;

#[tokio::test]
async fn test_init_and_views() -> Result<(), Box<dyn std::error::Error>> {
    let w = TreasuryTestWorkspaceBuilder::default().build().await?;

    assert_eq!(w.get_num_whitelisted().await?, 0);
    let alice = w.sandbox.dev_create_account().await?;
    assert!(!w.is_whitelisted(alice.id()).await?);
    assert!(w.whitelist_entry(alice.id()).await?.is_null());

    Ok(())
}

#[tokio::test]
async fn test_whitelist_and_transfer() -> Result<(), Box<dyn std::error::Error>> {
    let w = TreasuryTestWorkspaceBuilder::default().build().await?;
    let alice = w.sandbox.dev_create_account().await?;
    let limit = NearToken::from_near(10);

    let outcome = w.add_to_whitelist(&w.admin_dao, alice.id(), limit).await?;
    outcome_check(&outcome);
    assert!(w.is_whitelisted(alice.id()).await?);
    assert_eq!(w.get_num_whitelisted().await?, 1);
    assert_eq!(w.remaining_limit(alice.id()).await?, limit);

    // The spender transfers within the limit; the receiver gets the exact amount.
    let amount = NearToken::from_near(4);
    let alice_before = w.balance(&alice).await?;
    let treasury_before = w.balance(&w.treasury).await?;
    let outcome = w.transfer(&w.spender_dao, alice.id(), amount).await?;
    outcome_check(&outcome);

    let alice_after = w.balance(&alice).await?;
    assert_eq!(
        alice_after.as_yoctonear(),
        alice_before.as_yoctonear() + amount.as_yoctonear(),
        "The receiver should get the exact amount"
    );
    assert_almost_eq(
        w.balance(&w.treasury).await?,
        treasury_before.saturating_sub(amount),
        NearToken::from_millinear(10),
    );

    let remaining = w.remaining_limit(alice.id()).await?;
    assert_eq!(remaining, limit.saturating_sub(amount));

    // Spending the exact remaining allowance is allowed.
    let outcome = w.transfer(&w.spender_dao, alice.id(), remaining).await?;
    outcome_check(&outcome);
    assert_eq!(
        w.remaining_limit(alice.id()).await?,
        NearToken::from_yoctonear(0)
    );

    // The allowance is exhausted.
    let outcome = w
        .transfer(&w.spender_dao, alice.id(), NearToken::from_yoctonear(1))
        .await?;
    assert!(
        outcome.is_failure(),
        "Transfer over the limit should fail: {:#?}",
        outcome.outcomes()
    );

    Ok(())
}

#[tokio::test]
async fn test_transfer_restrictions() -> Result<(), Box<dyn std::error::Error>> {
    let w = TreasuryTestWorkspaceBuilder::default().build().await?;
    let alice = w.sandbox.dev_create_account().await?;
    let limit = NearToken::from_near(10);
    outcome_check(&w.add_to_whitelist(&w.admin_dao, alice.id(), limit).await?);

    // Only the spender can transfer, not the admin or the receiver.
    for account in [&w.admin_dao, &alice] {
        let outcome = w
            .transfer(account, alice.id(), NearToken::from_near(1))
            .await?;
        assert!(
            outcome.is_failure(),
            "Transfer by non-spender should fail: {:#?}",
            outcome.outcomes()
        );
    }

    // No transfers to non-whitelisted accounts.
    let bob = w.sandbox.dev_create_account().await?;
    let outcome = w
        .transfer(&w.spender_dao, bob.id(), NearToken::from_near(1))
        .await?;
    assert!(
        outcome.is_failure(),
        "Transfer to a non-whitelisted account should fail: {:#?}",
        outcome.outcomes()
    );

    // No zero transfers, no transfers over the limit.
    let outcome = w
        .transfer(&w.spender_dao, alice.id(), NearToken::from_yoctonear(0))
        .await?;
    assert!(outcome.is_failure(), "Zero transfer should fail");
    let outcome = w
        .transfer(
            &w.spender_dao,
            alice.id(),
            limit.saturating_add(NearToken::from_yoctonear(1)),
        )
        .await?;
    assert!(outcome.is_failure(), "Transfer over the limit should fail");

    assert_eq!(
        w.remaining_limit(alice.id()).await?,
        limit,
        "Failed transfers should not consume the allowance"
    );

    Ok(())
}

#[tokio::test]
async fn test_admin_management() -> Result<(), Box<dyn std::error::Error>> {
    let w = TreasuryTestWorkspaceBuilder::default().build().await?;
    let alice = w.sandbox.dev_create_account().await?;
    let limit = NearToken::from_near(10);

    // Only the admin manages the whitelist.
    for account in [&w.spender_dao, &alice] {
        let outcome = w.add_to_whitelist(account, alice.id(), limit).await?;
        assert!(
            outcome.is_failure(),
            "Whitelisting by non-admin should fail: {:#?}",
            outcome.outcomes()
        );
    }

    outcome_check(&w.add_to_whitelist(&w.admin_dao, alice.id(), limit).await?);

    // No duplicates, no whitelisting the treasury itself.
    let outcome = w.add_to_whitelist(&w.admin_dao, alice.id(), limit).await?;
    assert!(outcome.is_failure(), "Duplicate whitelisting should fail");
    let outcome = w
        .add_to_whitelist(&w.admin_dao, w.treasury.id(), limit)
        .await?;
    assert!(
        outcome.is_failure(),
        "Whitelisting the treasury itself should fail"
    );

    // `set_limit` overrides the remaining allowance.
    outcome_check(
        &w.transfer(&w.spender_dao, alice.id(), NearToken::from_near(4))
            .await?,
    );
    outcome_check(
        &w.set_limit(&w.admin_dao, alice.id(), NearToken::from_near(3))
            .await?,
    );
    assert_eq!(
        w.remaining_limit(alice.id()).await?,
        NearToken::from_near(3)
    );

    // Removal revokes the allowance entirely.
    outcome_check(&w.remove_from_whitelist(&w.admin_dao, alice.id()).await?);
    assert!(!w.is_whitelisted(alice.id()).await?);
    let outcome = w
        .transfer(&w.spender_dao, alice.id(), NearToken::from_near(1))
        .await?;
    assert!(
        outcome.is_failure(),
        "Transfer to a removed account should fail"
    );

    Ok(())
}

#[tokio::test]
async fn test_role_rotation() -> Result<(), Box<dyn std::error::Error>> {
    let w = TreasuryTestWorkspaceBuilder::default().build().await?;
    let alice = w.sandbox.dev_create_account().await?;
    outcome_check(
        &w.add_to_whitelist(&w.admin_dao, alice.id(), NearToken::from_near(10))
            .await?,
    );

    let new_spender = w.sandbox.dev_create_account().await?;
    let new_admin = w.sandbox.dev_create_account().await?;

    // Only the admin can rotate roles.
    let outcome = w
        .spender_dao
        .call(w.treasury.id(), "set_spender")
        .args_json(json!({ "spender_id": new_spender.id() }))
        .transact()
        .await?;
    assert!(
        outcome.is_failure(),
        "Role rotation by non-admin should fail"
    );

    let outcome = w
        .admin_dao
        .call(w.treasury.id(), "set_spender")
        .args_json(json!({ "spender_id": new_spender.id() }))
        .transact()
        .await?;
    outcome_check(&outcome);

    // The old spender lost the role, the new one can transfer.
    let outcome = w
        .transfer(&w.spender_dao, alice.id(), NearToken::from_near(1))
        .await?;
    assert!(outcome.is_failure(), "The old spender should be rejected");
    outcome_check(
        &w.transfer(&new_spender, alice.id(), NearToken::from_near(1))
            .await?,
    );

    // The admin hands over its own role and loses control.
    let outcome = w
        .admin_dao
        .call(w.treasury.id(), "set_admin")
        .args_json(json!({ "admin_id": new_admin.id() }))
        .transact()
        .await?;
    outcome_check(&outcome);
    let bob = w.sandbox.dev_create_account().await?;
    let outcome = w
        .add_to_whitelist(&w.admin_dao, bob.id(), NearToken::from_near(1))
        .await?;
    assert!(outcome.is_failure(), "The old admin should be rejected");
    outcome_check(
        &w.add_to_whitelist(&new_admin, bob.id(), NearToken::from_near(1))
            .await?,
    );

    Ok(())
}

#[tokio::test]
async fn test_failed_transfer_rolls_back_allowance() -> Result<(), Box<dyn std::error::Error>> {
    let w = TreasuryTestWorkspaceBuilder::default().build().await?;
    // A whitelisted account that does not exist on chain: the NEAR transfer
    // receipt fails and the refund callback must restore the limit.
    let ghost = AccountId::from_str("ghost.test.near").unwrap();
    let limit = NearToken::from_near(10);
    outcome_check(&w.add_to_whitelist(&w.admin_dao, &ghost, limit).await?);

    let treasury_before = w.balance(&w.treasury).await?;
    let outcome = w
        .transfer(&w.spender_dao, &ghost, NearToken::from_near(4))
        .await?;
    // The transaction itself succeeds (the callback handles the error), but the
    // transfer receipt fails.
    assert!(
        outcome.is_success(),
        "The callback should absorb the failure: {:#?}",
        outcome.outcomes()
    );
    assert!(
        !outcome.failures().is_empty(),
        "The transfer receipt should have failed"
    );

    // The allowance is restored and the funds stayed in the treasury.
    assert_eq!(w.remaining_limit(&ghost).await?, limit);
    assert_almost_eq(
        w.balance(&w.treasury).await?,
        treasury_before,
        NearToken::from_millinear(10),
    );

    Ok(())
}

#[tokio::test]
async fn test_limit_does_not_expire() -> Result<(), Box<dyn std::error::Error>> {
    let w = TreasuryTestWorkspaceBuilder::default().build().await?;
    let alice = w.sandbox.dev_create_account().await?;
    let limit = NearToken::from_near(10);
    outcome_check(&w.add_to_whitelist(&w.admin_dao, alice.id(), limit).await?);

    // Exhaust the allowance.
    outcome_check(&w.transfer(&w.spender_dao, alice.id(), limit).await?);
    let outcome = w
        .transfer(&w.spender_dao, alice.id(), NearToken::from_yoctonear(1))
        .await?;
    assert!(outcome.is_failure(), "The allowance should be exhausted");

    // The allowance never resets; only a raised limit extends it.
    outcome_check(
        &w.set_limit(&w.admin_dao, alice.id(), NearToken::from_near(5))
            .await?,
    );
    assert_eq!(
        w.remaining_limit(alice.id()).await?,
        NearToken::from_near(5)
    );

    outcome_check(
        &w.transfer(&w.spender_dao, alice.id(), NearToken::from_near(5))
            .await?,
    );
    assert_eq!(
        w.remaining_limit(alice.id()).await?,
        NearToken::from_yoctonear(0)
    );

    Ok(())
}
