//! End-to-end tests of the intended production topology:
//! each DAO acts on the spending account through its own dao-timelock.

mod setup;

use crate::setup::timelock_helpers::{add_to_whitelist_action, function_call};
use crate::setup::{TreasuryTestWorkspaceBuilder, outcome_check};
use near_sdk::NearToken;
use serde_json::json;

#[tokio::test]
async fn test_dao_to_treasury_flow() -> Result<(), Box<dyn std::error::Error>> {
    let w = TreasuryTestWorkspaceBuilder::default()
        .with_timelocks()
        .build()
        .await?;
    let execution_timelock = w.execution_timelock.as_ref().unwrap();
    let payment_timelock = w.payment_timelock.as_ref().unwrap();
    let alice = w.sandbox.dev_create_account().await?;
    let limit = NearToken::from_near(10);

    // The DAOs cannot touch the treasury directly: the roles belong to the timelocks.
    let outcome = w
        .add_to_whitelist(&w.execution_dao, alice.id(), limit)
        .await?;
    assert!(
        outcome.is_failure(),
        "The execution DAO should not hold the manager role directly"
    );
    let outcome = w
        .transfer(&w.payment_dao, alice.id(), NearToken::from_near(1))
        .await?;
    assert!(
        outcome.is_failure(),
        "The payment DAO should not hold the spender role directly"
    );

    // The execution DAO whitelists alice through its timelock.
    let outcome = w
        .dao_action(
            &w.execution_dao,
            execution_timelock,
            w.treasury.id(),
            "add_to_whitelist",
            add_to_whitelist_action(alice.id(), limit),
            NearToken::from_yoctonear(0),
            30,
        )
        .await?;
    outcome_check(&outcome);
    assert!(w.is_whitelisted(alice.id()).await?);

    // The payment DAO pays alice through its timelock.
    let amount = NearToken::from_near(4);
    let alice_before = w.balance(&alice).await?;
    let outcome = w
        .dao_action(
            &w.payment_dao,
            payment_timelock,
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
    let policy_timelock = w.policy_timelock.as_ref().unwrap();
    let execution_timelock = w.execution_timelock.as_ref().unwrap();
    let payment_timelock = w.payment_timelock.as_ref().unwrap();
    let alice = w.sandbox.dev_create_account().await?;
    let limit = NearToken::from_near(10);

    // Neither the payment nor the policy timelock can manage the whitelist:
    // the request executes but the inner call is rejected by the spending
    // account.
    for (dao, timelock) in [
        (&w.payment_dao, payment_timelock),
        (&w.policy_dao, policy_timelock),
    ] {
        let outcome = w
            .dao_action(
                dao,
                timelock,
                w.treasury.id(),
                "add_to_whitelist",
                add_to_whitelist_action(alice.id(), limit),
                NearToken::from_yoctonear(0),
                30,
            )
            .await?;
        assert!(
            outcome.is_failure(),
            "Whitelisting through a non-execution timelock should fail"
        );
        assert!(!w.is_whitelisted(alice.id()).await?);
    }

    // Neither the policy nor the execution timelock can transfer funds.
    for (dao, timelock) in [
        (&w.policy_dao, policy_timelock),
        (&w.execution_dao, execution_timelock),
    ] {
        let outcome = w
            .dao_action(
                dao,
                timelock,
                w.treasury.id(),
                "transfer",
                json!({ "receiver_id": alice.id(), "amount": NearToken::from_near(1) }),
                NearToken::from_yoctonear(0),
                50,
            )
            .await?;
        assert!(
            outcome.is_failure(),
            "Transfers through a non-payment timelock should fail"
        );
    }

    // The execution timelock cannot rotate roles.
    let outcome = w
        .dao_action(
            &w.execution_dao,
            execution_timelock,
            w.treasury.id(),
            "set_spender",
            json!({ "spender_id": w.execution_dao.id() }),
            NearToken::from_yoctonear(0),
            30,
        )
        .await?;
    assert!(
        outcome.is_failure(),
        "Role rotation through the execution timelock should fail"
    );

    Ok(())
}

#[tokio::test]
async fn test_guardian_cancels_malicious_transfer() -> Result<(), Box<dyn std::error::Error>> {
    let w = TreasuryTestWorkspaceBuilder::default()
        .with_timelocks()
        .build()
        .await?;
    let execution_timelock = w.execution_timelock.as_ref().unwrap();
    let payment_timelock = w.payment_timelock.as_ref().unwrap();
    let alice = w.sandbox.dev_create_account().await?;
    let limit = NearToken::from_near(10);

    let outcome = w
        .dao_action(
            &w.execution_dao,
            execution_timelock,
            w.treasury.id(),
            "add_to_whitelist",
            add_to_whitelist_action(alice.id(), limit),
            NearToken::from_yoctonear(0),
            30,
        )
        .await?;
    outcome_check(&outcome);

    // The payment DAO schedules a transfer, but a guardian cancels it during
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
            &w.payment_dao,
            payment_timelock,
            w.treasury.id(),
            vec![action],
            NearToken::from_yoctonear(0),
        )
        .await?;

    let outcome = w.cancel(&w.guardian, payment_timelock, request_id).await?;
    outcome_check(&outcome);

    // The cancelled request cannot be executed and no funds moved.
    let outcome = w
        .execute(&w.payment_dao, payment_timelock, request_id)
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
