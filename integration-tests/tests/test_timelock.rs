mod setup;

use crate::setup::timelock_helpers::function_call;
use crate::setup::{
    NS_IN_SECOND, TIMELOCK_DELAY_SECONDS, TreasuryTestWorkspaceBuilder, assert_almost_eq,
    outcome_check,
};
use near_sdk::NearToken;
use serde_json::json;

#[tokio::test]
async fn test_init_and_views() -> Result<(), Box<dyn std::error::Error>> {
    let w = TreasuryTestWorkspaceBuilder::default()
        .with_timelocks()
        .build()
        .await?;
    let timelock = w.admin_timelock.as_ref().unwrap();

    let dao: String = w
        .sandbox
        .view(timelock.id(), "get_dao")
        .args_json(json!({}))
        .await?
        .json()?;
    assert_eq!(&dao, w.admin_dao.id().as_str(), "Invalid DAO");

    let guardians: Vec<String> = w
        .sandbox
        .view(timelock.id(), "get_guardians")
        .args_json(json!({}))
        .await?
        .json()?;
    assert_eq!(guardians, vec![w.guardian.id().to_string()]);

    assert_eq!(
        w.get_delay_ns(timelock).await?,
        TIMELOCK_DELAY_SECONDS * NS_IN_SECOND
    );
    assert_eq!(w.get_num_requests(timelock).await?, 0);

    Ok(())
}

#[tokio::test]
async fn test_schedule_and_execute_after_delay() -> Result<(), Box<dyn std::error::Error>> {
    let w = TreasuryTestWorkspaceBuilder::default()
        .with_timelocks()
        .build()
        .await?;
    let timelock = w.admin_timelock.as_ref().unwrap();
    let alice = w.sandbox.dev_create_account().await?;

    let action = function_call(
        "add_to_whitelist",
        json!({
            "account_id": alice.id(),
            "yearly_limit": NearToken::from_near(10),
        }),
        NearToken::from_yoctonear(0),
        30,
    );

    let before = w.block_timestamp().await?;
    let request_id = w
        .schedule(
            &w.admin_dao,
            timelock,
            w.treasury.id(),
            vec![action],
            NearToken::from_yoctonear(0),
        )
        .await?;
    assert_eq!(request_id, 0);
    assert_eq!(w.get_num_requests(timelock).await?, 1);

    let request = w.get_request(timelock, request_id).await?;
    assert_eq!(
        request["receiver_id"].as_str().unwrap(),
        w.treasury.id().as_str()
    );
    assert_eq!(
        request["funder_id"].as_str().unwrap(),
        w.admin_dao.id().as_str()
    );
    let execute_after: u64 = request["execute_after"].as_str().unwrap().parse()?;
    assert!(
        execute_after >= before + TIMELOCK_DELAY_SECONDS * NS_IN_SECOND,
        "execute_after should be at least the delay in the future"
    );

    // Executing before the delay has passed must fail.
    let outcome = w.execute(&w.admin_dao, timelock, request_id).await?;
    assert!(
        outcome.is_failure(),
        "Executing before the delay should fail: {:#?}",
        outcome.outcomes()
    );
    assert_eq!(w.get_num_requests(timelock).await?, 1);
    assert!(!w.is_whitelisted(alice.id()).await?);

    // After the delay, anyone can execute.
    w.fast_forward_to_executable(timelock, request_id).await?;
    let anyone = w.sandbox.dev_create_account().await?;
    let outcome = w.execute(&anyone, timelock, request_id).await?;
    outcome_check(&outcome);

    assert!(w.is_whitelisted(alice.id()).await?);
    assert_eq!(w.get_num_requests(timelock).await?, 0);
    assert!(w.get_request(timelock, request_id).await?.is_null());

    // The request is removed: executing it again must fail.
    let outcome = w.execute(&anyone, timelock, request_id).await?;
    assert!(
        outcome.is_failure(),
        "Executing twice should fail: {:#?}",
        outcome.outcomes()
    );

    Ok(())
}

#[tokio::test]
async fn test_only_dao_can_schedule() -> Result<(), Box<dyn std::error::Error>> {
    let w = TreasuryTestWorkspaceBuilder::default()
        .with_timelocks()
        .build()
        .await?;
    let timelock = w.admin_timelock.as_ref().unwrap();

    let action = function_call("get_period_info", json!({}), NearToken::from_yoctonear(0), 10);

    // Neither a guardian nor the other DAO can schedule.
    for account in [&w.guardian, &w.spender_dao] {
        let outcome = w
            .schedule_raw(
                account,
                timelock,
                w.treasury.id(),
                vec![action.clone()],
                NearToken::from_yoctonear(0),
            )
            .await?;
        assert!(
            outcome.is_failure(),
            "Scheduling by non-DAO should fail: {:#?}",
            outcome.outcomes()
        );
    }
    assert_eq!(w.get_num_requests(timelock).await?, 0);

    // A request with no actions is rejected.
    let outcome = w
        .schedule_raw(
            &w.admin_dao,
            timelock,
            w.treasury.id(),
            vec![],
            NearToken::from_yoctonear(0),
        )
        .await?;
    assert!(
        outcome.is_failure(),
        "Scheduling an empty request should fail: {:#?}",
        outcome.outcomes()
    );

    Ok(())
}

#[tokio::test]
async fn test_deposit_escrow_and_cancel_refund() -> Result<(), Box<dyn std::error::Error>> {
    let w = TreasuryTestWorkspaceBuilder::default()
        .with_timelocks()
        .build()
        .await?;
    let timelock = w.admin_timelock.as_ref().unwrap();

    let deposit = NearToken::from_near(1);
    let action = function_call("some_method", json!({}), deposit, 10);

    // The attached deposit must exactly match the total action deposits.
    let outcome = w
        .schedule_raw(
            &w.admin_dao,
            timelock,
            w.treasury.id(),
            vec![action.clone()],
            NearToken::from_yoctonear(0),
        )
        .await?;
    assert!(
        outcome.is_failure(),
        "Scheduling with a wrong deposit should fail: {:#?}",
        outcome.outcomes()
    );

    // Schedule with the correct deposit: it is held in escrow by the timelock.
    let balance_before = w.balance(&w.admin_dao).await?;
    let request_id = w
        .schedule(
            &w.admin_dao,
            timelock,
            w.treasury.id(),
            vec![action],
            deposit,
        )
        .await?;
    let balance_escrowed = w.balance(&w.admin_dao).await?;
    assert_almost_eq(
        balance_escrowed,
        balance_before.saturating_sub(deposit),
        NearToken::from_millinear(50),
    );

    // A random account cannot cancel.
    let anyone = w.sandbox.dev_create_account().await?;
    let outcome = w.cancel(&anyone, timelock, request_id).await?;
    assert!(
        outcome.is_failure(),
        "Cancelling by a random account should fail: {:#?}",
        outcome.outcomes()
    );

    // A guardian cancels and the deposit is refunded to the funder (the DAO).
    let outcome = w.cancel(&w.guardian, timelock, request_id).await?;
    outcome_check(&outcome);
    assert_eq!(w.get_num_requests(timelock).await?, 0);
    let balance_refunded = w.balance(&w.admin_dao).await?;
    assert_almost_eq(
        balance_refunded,
        balance_before,
        NearToken::from_millinear(50),
    );

    // The cancelled request cannot be executed.
    w.fast_forward(
        w.block_timestamp().await? + TIMELOCK_DELAY_SECONDS * NS_IN_SECOND,
        TIMELOCK_DELAY_SECONDS,
        20,
    )
    .await?;
    let outcome = w.execute(&w.admin_dao, timelock, request_id).await?;
    assert!(
        outcome.is_failure(),
        "Executing a cancelled request should fail: {:#?}",
        outcome.outcomes()
    );

    Ok(())
}

#[tokio::test]
async fn test_config_change_via_scheduled_request() -> Result<(), Box<dyn std::error::Error>> {
    let w = TreasuryTestWorkspaceBuilder::default()
        .with_timelocks()
        .build()
        .await?;
    let timelock = w.admin_timelock.as_ref().unwrap();

    // Neither the DAO nor a guardian can change the config directly.
    let new_delay_ns = NS_IN_SECOND;
    for account in [&w.admin_dao, &w.guardian] {
        let outcome = account
            .call(timelock.id(), "set_delay")
            .args_json(json!({ "delay_ns": new_delay_ns.to_string() }))
            .transact()
            .await?;
        assert!(
            outcome.is_failure(),
            "Direct config change should fail: {:#?}",
            outcome.outcomes()
        );
    }

    // The config is changed by the timelock calling itself via a scheduled request.
    let outcome = w
        .dao_action(
            &w.admin_dao,
            timelock,
            timelock.id(),
            "set_delay",
            json!({ "delay_ns": new_delay_ns.to_string() }),
            NearToken::from_yoctonear(0),
            10,
        )
        .await?;
    outcome_check(&outcome);
    assert_eq!(w.get_delay_ns(timelock).await?, new_delay_ns);

    // Same pipeline for the guardians list.
    let new_guardian = w.sandbox.dev_create_account().await?;
    let outcome = w
        .dao_action(
            &w.admin_dao,
            timelock,
            timelock.id(),
            "set_guardians",
            json!({ "guardians": &[new_guardian.id()] }),
            NearToken::from_yoctonear(0),
            10,
        )
        .await?;
    outcome_check(&outcome);
    let guardians: Vec<String> = w
        .sandbox
        .view(timelock.id(), "get_guardians")
        .args_json(json!({}))
        .await?
        .json()?;
    assert_eq!(guardians, vec![new_guardian.id().to_string()]);

    // The old guardian lost the ability to cancel.
    let action = function_call("get_period_info", json!({}), NearToken::from_yoctonear(0), 10);
    let request_id = w
        .schedule(
            &w.admin_dao,
            timelock,
            w.treasury.id(),
            vec![action],
            NearToken::from_yoctonear(0),
        )
        .await?;
    let outcome = w.cancel(&w.guardian, timelock, request_id).await?;
    assert!(
        outcome.is_failure(),
        "The removed guardian should not be able to cancel: {:#?}",
        outcome.outcomes()
    );
    let outcome = w.cancel(&new_guardian, timelock, request_id).await?;
    outcome_check(&outcome);

    Ok(())
}
