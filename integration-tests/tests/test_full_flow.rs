//! End-to-end tests of the intended production topology: the policy and
//! execution DAOs act on the spending account through their own dao-timelock,
//! while the payment DAO holds the spender role directly — spending is not
//! timelocked, it is bounded by the whitelist and the limits alone.

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
    let alice = w.sandbox.dev_create_account().await?;
    let limit = NearToken::from_near(10);

    // The execution DAO cannot touch the whitelist directly: the manager role
    // belongs to its timelock.
    let outcome = w
        .add_to_whitelist(&w.execution_dao, alice.id(), limit)
        .await?;
    assert!(
        outcome.is_failure(),
        "The execution DAO should not hold the manager role directly"
    );
    // The payment DAO does hold the spender role directly, but there is nothing
    // it may pay yet: alice is not whitelisted.
    let outcome = w
        .transfer(&w.payment_dao, alice.id(), NearToken::from_near(1))
        .await?;
    assert!(
        outcome.is_failure(),
        "A transfer to a non-whitelisted account should fail"
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

    // The payment DAO pays alice directly, with no timelock and no delay.
    let amount = NearToken::from_near(4);
    let alice_before = w.balance(&alice).await?;
    let outcome = w.transfer(&w.payment_dao, alice.id(), amount).await?;
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
async fn test_role_separation_between_roles() -> Result<(), Box<dyn std::error::Error>> {
    let w = TreasuryTestWorkspaceBuilder::default()
        .with_timelocks()
        .build()
        .await?;
    let policy_timelock = w.policy_timelock.as_ref().unwrap();
    let execution_timelock = w.execution_timelock.as_ref().unwrap();
    let alice = w.sandbox.dev_create_account().await?;
    let limit = NearToken::from_near(10);

    // The spender cannot whitelist itself a recipient: the payment DAO has no
    // manager rights, even though it calls the spending account directly.
    let outcome = w
        .add_to_whitelist(&w.payment_dao, alice.id(), limit)
        .await?;
    assert!(
        outcome.is_failure(),
        "Whitelisting by the payment DAO should fail"
    );
    assert!(!w.is_whitelisted(alice.id()).await?);

    // The policy timelock cannot manage the whitelist either: the request
    // executes but the inner call is rejected by the spending account.
    let outcome = w
        .dao_action(
            &w.policy_dao,
            policy_timelock,
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
            "Transfers by a non-spender should fail"
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
async fn test_guardian_cancels_malicious_whitelisting() -> Result<(), Box<dyn std::error::Error>> {
    let w = TreasuryTestWorkspaceBuilder::default()
        .with_timelocks()
        .build()
        .await?;
    let execution_timelock = w.execution_timelock.as_ref().unwrap();
    let alice = w.sandbox.dev_create_account().await?;
    let limit = NearToken::from_near(10);

    // Payouts are not timelocked, so the guardian window sits on the whitelist:
    // the execution DAO schedules alice's whitelisting and a guardian cancels
    // it during the delay.
    let action = function_call(
        "add_to_whitelist",
        add_to_whitelist_action(alice.id(), limit),
        NearToken::from_yoctonear(0),
        30,
    );
    let alice_before = w.balance(&alice).await?;
    let request_id = w
        .schedule(
            &w.execution_dao,
            execution_timelock,
            w.treasury.id(),
            vec![action],
            NearToken::from_yoctonear(0),
        )
        .await?;

    let outcome = w
        .cancel(&w.guardian, execution_timelock, request_id)
        .await?;
    outcome_check(&outcome);

    // The cancelled request cannot be executed and alice stays unwhitelisted.
    let outcome = w
        .execute(&w.execution_dao, execution_timelock, request_id)
        .await?;
    assert!(
        outcome.is_failure(),
        "Executing a cancelled request should fail"
    );
    assert!(!w.is_whitelisted(alice.id()).await?);

    // So the payment DAO cannot pay alice, even though it spends untimelocked.
    let outcome = w.transfer(&w.payment_dao, alice.id(), limit).await?;
    assert!(
        outcome.is_failure(),
        "The spender should not be able to pay a cancelled whitelisting"
    );
    assert_eq!(
        w.balance(&alice).await?.as_yoctonear(),
        alice_before.as_yoctonear(),
        "No funds should have moved"
    );

    Ok(())
}
