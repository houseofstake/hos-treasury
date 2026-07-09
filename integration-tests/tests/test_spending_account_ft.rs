mod setup;

use crate::setup::{TreasuryTestWorkspaceBuilder, outcome_check};
use near_sdk::NearToken;

#[tokio::test]
async fn test_ft_whitelist_and_transfer() -> Result<(), Box<dyn std::error::Error>> {
    let w = TreasuryTestWorkspaceBuilder::default().build().await?;
    let token = w
        .deploy_ft_and_fund_treasury(NearToken::from_near(20))
        .await?;
    let alice = w.sandbox.dev_create_account().await?;
    w.ft_register(&token, alice.id()).await?;
    let limit = NearToken::from_near(10).as_yoctonear();

    let outcome = w
        .add_to_ft_whitelist(&w.admin_dao, alice.id(), token.id(), limit)
        .await?;
    outcome_check(&outcome);
    assert!(w.is_ft_whitelisted(alice.id(), token.id()).await?);
    assert_eq!(w.get_num_whitelisted().await?, 1);
    // The token entry grants no native NEAR allowance.
    assert!(!w.is_whitelisted(alice.id()).await?);
    assert_eq!(w.ft_remaining_limit(alice.id(), token.id()).await?, limit);

    // The spender transfers within the limit; the receiver gets the exact amount.
    let amount = NearToken::from_near(4).as_yoctonear();
    let treasury_ft_before = w.ft_balance(&token, w.treasury.id()).await?;
    let outcome = w
        .transfer_ft(&w.spender_dao, alice.id(), token.id(), amount)
        .await?;
    outcome_check(&outcome);

    assert_eq!(w.ft_balance(&token, alice.id()).await?, amount);
    assert_eq!(
        w.ft_balance(&token, w.treasury.id()).await?,
        treasury_ft_before - amount
    );
    let remaining = w.ft_remaining_limit(alice.id(), token.id()).await?;
    assert_eq!(remaining, limit - amount);

    // Spending the exact remaining allowance is allowed.
    let outcome = w
        .transfer_ft(&w.spender_dao, alice.id(), token.id(), remaining)
        .await?;
    outcome_check(&outcome);
    assert_eq!(w.ft_balance(&token, alice.id()).await?, limit);
    assert_eq!(w.ft_remaining_limit(alice.id(), token.id()).await?, 0);

    // The allowance is exhausted.
    let outcome = w
        .transfer_ft(&w.spender_dao, alice.id(), token.id(), 1)
        .await?;
    assert!(
        outcome.is_failure(),
        "Transfer over the limit should fail: {:#?}",
        outcome.outcomes()
    );

    Ok(())
}

#[tokio::test]
async fn test_ft_transfer_restrictions() -> Result<(), Box<dyn std::error::Error>> {
    let w = TreasuryTestWorkspaceBuilder::default().build().await?;
    let token = w
        .deploy_ft_and_fund_treasury(NearToken::from_near(20))
        .await?;
    let alice = w.sandbox.dev_create_account().await?;
    w.ft_register(&token, alice.id()).await?;
    let limit = NearToken::from_near(10).as_yoctonear();
    outcome_check(
        &w.add_to_ft_whitelist(&w.admin_dao, alice.id(), token.id(), limit)
            .await?,
    );

    // Only the spender can transfer, not the admin or the receiver.
    for account in [&w.admin_dao, &alice] {
        let outcome = w.transfer_ft(account, alice.id(), token.id(), 1).await?;
        assert!(
            outcome.is_failure(),
            "Transfer by non-spender should fail: {:#?}",
            outcome.outcomes()
        );
    }

    // No transfers to accounts not whitelisted for this token, even if they
    // are whitelisted for NEAR.
    let bob = w.sandbox.dev_create_account().await?;
    outcome_check(
        &w.add_to_whitelist(&w.admin_dao, bob.id(), NearToken::from_near(10))
            .await?,
    );
    let outcome = w
        .transfer_ft(&w.spender_dao, bob.id(), token.id(), 1)
        .await?;
    assert!(
        outcome.is_failure(),
        "Transfer to a non-whitelisted pair should fail: {:#?}",
        outcome.outcomes()
    );

    // No zero transfers, no transfers over the limit.
    let outcome = w
        .transfer_ft(&w.spender_dao, alice.id(), token.id(), 0)
        .await?;
    assert!(outcome.is_failure(), "Zero transfer should fail");
    let outcome = w
        .transfer_ft(&w.spender_dao, alice.id(), token.id(), limit + 1)
        .await?;
    assert!(outcome.is_failure(), "Transfer over the limit should fail");

    assert_eq!(
        w.ft_remaining_limit(alice.id(), token.id()).await?,
        limit,
        "Failed transfers should not consume the allowance"
    );

    Ok(())
}

#[tokio::test]
async fn test_ft_admin_management() -> Result<(), Box<dyn std::error::Error>> {
    let w = TreasuryTestWorkspaceBuilder::default().build().await?;
    let token = w
        .deploy_ft_and_fund_treasury(NearToken::from_near(20))
        .await?;
    let alice = w.sandbox.dev_create_account().await?;
    w.ft_register(&token, alice.id()).await?;
    let limit = NearToken::from_near(10).as_yoctonear();

    // Only the admin manages the FT whitelist.
    for account in [&w.spender_dao, &alice] {
        let outcome = w
            .add_to_ft_whitelist(account, alice.id(), token.id(), limit)
            .await?;
        assert!(
            outcome.is_failure(),
            "Whitelisting by non-admin should fail: {:#?}",
            outcome.outcomes()
        );
    }

    outcome_check(
        &w.add_to_ft_whitelist(&w.admin_dao, alice.id(), token.id(), limit)
            .await?,
    );

    // No duplicates, no whitelisting the treasury itself.
    let outcome = w
        .add_to_ft_whitelist(&w.admin_dao, alice.id(), token.id(), limit)
        .await?;
    assert!(outcome.is_failure(), "Duplicate whitelisting should fail");
    let outcome = w
        .add_to_ft_whitelist(&w.admin_dao, w.treasury.id(), token.id(), limit)
        .await?;
    assert!(
        outcome.is_failure(),
        "Whitelisting the treasury itself should fail"
    );

    // `set_limit` overrides the remaining allowance.
    outcome_check(
        &w.transfer_ft(
            &w.spender_dao,
            alice.id(),
            token.id(),
            NearToken::from_near(4).as_yoctonear(),
        )
        .await?,
    );
    outcome_check(
        &w.set_ft_limit(
            &w.admin_dao,
            alice.id(),
            token.id(),
            NearToken::from_near(3).as_yoctonear(),
        )
        .await?,
    );
    assert_eq!(
        w.ft_remaining_limit(alice.id(), token.id()).await?,
        NearToken::from_near(3).as_yoctonear()
    );

    // Removal revokes the allowance entirely.
    outcome_check(
        &w.remove_from_ft_whitelist(&w.admin_dao, alice.id(), token.id())
            .await?,
    );
    assert!(!w.is_ft_whitelisted(alice.id(), token.id()).await?);
    let outcome = w
        .transfer_ft(&w.spender_dao, alice.id(), token.id(), 1)
        .await?;
    assert!(
        outcome.is_failure(),
        "Transfer to a removed pair should fail"
    );

    Ok(())
}

#[tokio::test]
async fn test_ft_failed_transfer_rolls_back_allowance() -> Result<(), Box<dyn std::error::Error>> {
    let w = TreasuryTestWorkspaceBuilder::default().build().await?;
    let token = w
        .deploy_ft_and_fund_treasury(NearToken::from_near(20))
        .await?;
    // A whitelisted receiver that is not registered with the token: the
    // `ft_transfer` receipt fails and the callback must restore the limit
    // while the tokens stay on the treasury's balance.
    let bob = w.sandbox.dev_create_account().await?;
    let limit = NearToken::from_near(10).as_yoctonear();
    outcome_check(
        &w.add_to_ft_whitelist(&w.admin_dao, bob.id(), token.id(), limit)
            .await?,
    );

    let treasury_ft_before = w.ft_balance(&token, w.treasury.id()).await?;
    let outcome = w
        .transfer_ft(
            &w.spender_dao,
            bob.id(),
            token.id(),
            NearToken::from_near(4).as_yoctonear(),
        )
        .await?;
    // The transaction itself succeeds (the callback handles the error), but the
    // `ft_transfer` receipt fails.
    assert!(
        outcome.is_success(),
        "The callback should absorb the failure: {:#?}",
        outcome.outcomes()
    );
    assert!(
        !outcome.failures().is_empty(),
        "The ft_transfer receipt should have failed"
    );

    // The allowance is restored and the tokens stayed in the treasury.
    assert_eq!(w.ft_remaining_limit(bob.id(), token.id()).await?, limit);
    assert_eq!(
        w.ft_balance(&token, w.treasury.id()).await?,
        treasury_ft_before
    );

    Ok(())
}
