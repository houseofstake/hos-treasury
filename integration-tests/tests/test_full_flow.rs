//! End-to-end tests of the intended production topology:
//! each DAO acts on the spending account through its own dao-timelock.

mod setup;

use crate::setup::timelock_helpers::function_call;
use crate::setup::{TreasuryTestWorkspaceBuilder, outcome_check};
use near_sdk::NearToken;
use near_sdk::json_types::U128;
use serde_json::json;

#[tokio::test]
async fn test_dao_to_treasury_flow() -> Result<(), Box<dyn std::error::Error>> {
    let w = TreasuryTestWorkspaceBuilder::default()
        .with_timelocks()
        .build()
        .await?;
    let admin_timelock = w.admin_timelock.as_ref().unwrap();
    let spender_timelock = w.spender_timelock.as_ref().unwrap();
    let alice = w.sandbox.dev_create_account().await?;
    let limit = NearToken::from_near(10);

    // The DAOs cannot touch the treasury directly: the roles belong to the timelocks.
    let outcome = w.add_to_whitelist(&w.admin_dao, alice.id(), limit).await?;
    assert!(
        outcome.is_failure(),
        "The admin DAO should not hold the admin role directly"
    );
    let outcome = w
        .transfer(&w.spender_dao, alice.id(), NearToken::from_near(1))
        .await?;
    assert!(
        outcome.is_failure(),
        "The spender DAO should not hold the spender role directly"
    );

    // The admin DAO whitelists alice through its timelock.
    let outcome = w
        .dao_action(
            &w.admin_dao,
            admin_timelock,
            w.treasury.id(),
            "add_to_whitelist",
            json!({
                "account_id": alice.id(),
                "token_id": null,
                "limit": U128(limit.as_yoctonear()),
            }),
            NearToken::from_yoctonear(0),
            30,
        )
        .await?;
    outcome_check(&outcome);
    assert!(w.is_whitelisted(alice.id()).await?);

    // The spender DAO pays alice through its timelock.
    let amount = NearToken::from_near(4);
    let alice_before = w.balance(&alice).await?;
    let outcome = w
        .dao_action(
            &w.spender_dao,
            spender_timelock,
            w.treasury.id(),
            "transfer",
            json!({ "receiver_id": alice.id(), "amount": amount }),
            NearToken::from_yoctonear(0),
            50,
        )
        .await?;
    outcome_check(&outcome);

    assert_eq!(
        w.balance(&alice).await?.as_yoctonear(),
        alice_before.as_yoctonear() + amount.as_yoctonear(),
        "The receiver should get the exact amount"
    );
    assert_eq!(
        w.remaining_limit(alice.id()).await?,
        limit.saturating_sub(amount)
    );

    Ok(())
}

#[tokio::test]
async fn test_role_separation_between_timelocks() -> Result<(), Box<dyn std::error::Error>> {
    let w = TreasuryTestWorkspaceBuilder::default()
        .with_timelocks()
        .build()
        .await?;
    let admin_timelock = w.admin_timelock.as_ref().unwrap();
    let spender_timelock = w.spender_timelock.as_ref().unwrap();
    let alice = w.sandbox.dev_create_account().await?;
    let limit = NearToken::from_near(10);

    // The spender's timelock cannot perform admin actions: the request executes
    // but the inner call is rejected by the spending account.
    let outcome = w
        .dao_action(
            &w.spender_dao,
            spender_timelock,
            w.treasury.id(),
            "add_to_whitelist",
            json!({
                "account_id": alice.id(),
                "token_id": null,
                "limit": U128(limit.as_yoctonear()),
            }),
            NearToken::from_yoctonear(0),
            30,
        )
        .await?;
    assert!(
        outcome.is_failure(),
        "Admin actions through the spender timelock should fail"
    );
    assert!(!w.is_whitelisted(alice.id()).await?);

    // The admin's timelock cannot transfer funds.
    let outcome = w
        .dao_action(
            &w.admin_dao,
            admin_timelock,
            w.treasury.id(),
            "transfer",
            json!({ "receiver_id": alice.id(), "amount": NearToken::from_near(1) }),
            NearToken::from_yoctonear(0),
            50,
        )
        .await?;
    assert!(
        outcome.is_failure(),
        "Transfers through the admin timelock should fail"
    );

    Ok(())
}

#[tokio::test]
async fn test_guardian_cancels_malicious_transfer() -> Result<(), Box<dyn std::error::Error>> {
    let w = TreasuryTestWorkspaceBuilder::default()
        .with_timelocks()
        .build()
        .await?;
    let admin_timelock = w.admin_timelock.as_ref().unwrap();
    let spender_timelock = w.spender_timelock.as_ref().unwrap();
    let alice = w.sandbox.dev_create_account().await?;
    let limit = NearToken::from_near(10);

    let outcome = w
        .dao_action(
            &w.admin_dao,
            admin_timelock,
            w.treasury.id(),
            "add_to_whitelist",
            json!({
                "account_id": alice.id(),
                "token_id": null,
                "limit": U128(limit.as_yoctonear()),
            }),
            NearToken::from_yoctonear(0),
            30,
        )
        .await?;
    outcome_check(&outcome);

    // The spender DAO schedules a transfer, but a guardian cancels it during
    // the timelock delay.
    let action = function_call(
        "transfer",
        json!({ "receiver_id": alice.id(), "amount": limit }),
        NearToken::from_yoctonear(0),
        50,
    );
    let alice_before = w.balance(&alice).await?;
    let request_id = w
        .schedule(
            &w.spender_dao,
            spender_timelock,
            w.treasury.id(),
            vec![action],
            NearToken::from_yoctonear(0),
        )
        .await?;

    let outcome = w.cancel(&w.guardian, spender_timelock, request_id).await?;
    outcome_check(&outcome);

    // The cancelled request cannot be executed and no funds moved.
    let outcome = w
        .execute(&w.spender_dao, spender_timelock, request_id)
        .await?;
    assert!(
        outcome.is_failure(),
        "Executing a cancelled request should fail"
    );
    assert_eq!(
        w.balance(&alice).await?.as_yoctonear(),
        alice_before.as_yoctonear(),
        "No funds should have moved"
    );
    assert_eq!(w.remaining_limit(alice.id()).await?, limit);

    Ok(())
}
