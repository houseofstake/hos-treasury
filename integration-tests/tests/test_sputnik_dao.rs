//! Integration tests of a real SputnikDAO v2 (2-of-3 council) driving the
//! dao-timelock, with a DAO policy that forces every policy upgrade through
//! the timelock. See `setup/sputnik_helpers.rs` for the topology.

mod setup;

use crate::setup::sputnik_helpers::{
    PROPOSAL_BOND, dao_policy, schedule_policy_change_kind, setup_sputnik_workspace,
};
use crate::setup::{assert_almost_eq, outcome_check};
use near_sdk::NearToken;
use serde_json::json;

#[tokio::test]
async fn test_council_2_of_3_upgrades_policy_through_timelock()
-> Result<(), Box<dyn std::error::Error>> {
    let w = setup_sputnik_workspace().await?;
    let new_bond = NearToken::from_near(2);
    let new_policy = dao_policy(&w.council, w.timelock.id(), new_bond);

    // The council proposes a FunctionCall that schedules the policy change
    // through the timelock: the timelock will add the ChangePolicy proposal
    // and approve whatever id the DAO assigns to it.
    let outer_id = w.get_last_proposal_id().await?;
    let outer_kind =
        schedule_policy_change_kind(w.timelock.id(), w.dao.id(), &new_policy, PROPOSAL_BOND);
    let proposal_id = w
        .add_proposal(&w.council[0], &outer_kind, PROPOSAL_BOND)
        .await?;
    assert_eq!(proposal_id, outer_id);

    // One approval out of three is not enough: nothing is scheduled.
    let outcome = w
        .act_proposal(&w.council[0], proposal_id, "VoteApprove", &outer_kind)
        .await?;
    outcome_check(&outcome);
    assert_eq!(w.get_proposal_status(proposal_id).await?, "InProgress");
    assert_eq!(w.get_num_requests().await?, 0);

    // The second approval reaches the 2-of-3 threshold and the DAO calls
    // `timelock.schedule`, escrowing the bond for the inner proposal.
    let outcome = w
        .act_proposal(&w.council[1], proposal_id, "VoteApprove", &outer_kind)
        .await?;
    outcome_check(&outcome);
    assert_eq!(w.get_proposal_status(proposal_id).await?, "Approved");
    assert_eq!(w.get_num_requests().await?, 1);
    let request = w.get_request(0).await?;
    assert_eq!(
        request["receiver_id"].as_str().unwrap(),
        w.dao.id().as_str()
    );
    assert_eq!(request["funder_id"].as_str().unwrap(), w.dao.id().as_str());

    // A vote after finalization is rejected.
    let outcome = w
        .act_proposal(&w.council[2], proposal_id, "VoteApprove", &outer_kind)
        .await?;
    assert!(
        outcome.is_failure(),
        "Voting on a finalized proposal should fail: {:#?}",
        outcome.outcomes()
    );

    // After the delay, anyone executes
    w.fast_forward_to_executable(0).await?;
    let anyone = w.sandbox.dev_create_account().await?;
    let outcome = w.execute_request(&anyone, 0).await?;
    outcome_check(&outcome);

    let inner_id = w.get_last_proposal_id().await? - 1;
    assert_eq!(w.get_proposal_status(inner_id).await?, "Approved");
    let policy = w.get_policy().await?;
    assert_eq!(policy["proposal_bond"], json!(new_bond));
    assert_eq!(policy["roles"].as_array().unwrap().len(), 2);

    // The new policy is live: the old bond is now rejected, the new one works.
    let some_kind = json!({
        "FunctionCall": {
            "receiver_id": w.timelock.id(),
            "actions": [],
        }
    });
    let outcome = w
        .add_proposal_raw(&w.council[0], &some_kind, PROPOSAL_BOND)
        .await?;
    assert!(
        outcome.is_failure(),
        "The old bond should be rejected under the new policy: {:#?}",
        outcome.outcomes()
    );
    w.add_proposal(&w.council[0], &some_kind, new_bond).await?;

    Ok(())
}

#[tokio::test]
async fn test_policy_change_with_concurrent_proposals() -> Result<(), Box<dyn std::error::Error>> {
    let w = setup_sputnik_workspace().await?;
    let new_bond = NearToken::from_near(2);
    let new_policy = dao_policy(&w.council, w.timelock.id(), new_bond);

    let request_id = w.schedule_policy_change_via_council(&new_policy).await?;

    // Other proposals land on the DAO while the policy change is timelocked,
    // shifting the proposal id sequence. The inner ChangePolicy proposal must
    // still be created and approved: the timelock votes on the id returned by
    // add_proposal instead of an id predicted at schedule time.
    let unrelated_kind = json!({
        "FunctionCall": {
            "receiver_id": w.timelock.id(),
            "actions": [],
        }
    });
    for _ in 0..2 {
        w.add_proposal(&w.council[0], &unrelated_kind, PROPOSAL_BOND)
            .await?;
    }
    let last_before_execute = w.get_last_proposal_id().await?;

    w.fast_forward_to_executable(request_id).await?;
    let anyone = w.sandbox.dev_create_account().await?;
    let outcome = w.execute_request(&anyone, request_id).await?;
    outcome_check(&outcome);

    // The inner proposal took the next free id after the interleaved ones.
    let inner_id = w.get_last_proposal_id().await? - 1;
    assert_eq!(inner_id, last_before_execute);
    assert_eq!(w.get_proposal_status(inner_id).await?, "Approved");
    assert_eq!(w.get_policy().await?["proposal_bond"], json!(new_bond));

    Ok(())
}

#[tokio::test]
async fn test_failed_inner_proposal_refunds_bond() -> Result<(), Box<dyn std::error::Error>> {
    let w = setup_sputnik_workspace().await?;
    let dao_balance_before = w.balance(&w.dao).await?;
    let timelock_before = w.balance(&w.timelock).await?;
    let proposer_before = w.balance(&w.council[0]).await?;

    // The proposer schedules a failing proposal update.
    let bad_inner_kind = json!({
        "FunctionCall": {
            "receiver_id": w.guardian.id(),
            "actions": [],
        }
    });
    let request_id = w.schedule_proposal_via_council(&bad_inner_kind).await?;
    let last_proposal_id = w.get_last_proposal_id().await?;

    w.fast_forward_to_executable(request_id).await?;
    let anyone = w.sandbox.dev_create_account().await?;
    let anyone_before = w.balance(&anyone).await?;
    let outcome = w.execute_request(&anyone, request_id).await?;

    // add_proposal fails: the callback refunds the bond instead of voting.
    assert!(
        outcome.is_success(),
        "Execute should succeed even when add_proposal fails: {:#?}",
        outcome.outcomes()
    );
    assert_eq!(
        outcome.failures().len(),
        1,
        "Only the add_proposal receipt should fail: {:#?}",
        outcome.outcomes()
    );

    // No proposal was created and the request is consumed.
    assert_eq!(w.get_last_proposal_id().await?, last_proposal_id);
    assert_eq!(w.get_num_requests().await?, 0);

    let dao_after = w.balance(&w.dao).await?;
    let timelock_after = w.balance(&w.timelock).await?;
    assert!(
        dao_after >= dao_balance_before,
        "DAO must recover its full bond: after {} < before {}",
        dao_after.exact_amount_display(),
        dao_balance_before.exact_amount_display(),
    );
    assert!(
        timelock_after >= timelock_before,
        "timelock must not keep the bond: after {} < before {}",
        timelock_after.exact_amount_display(),
        timelock_before.exact_amount_display(),
    );

    // No balance must change.
    assert_almost_eq(dao_after, dao_balance_before, NearToken::from_millinear(2));
    assert_almost_eq(
        timelock_after,
        timelock_before,
        NearToken::from_millinear(1),
    );
    assert_almost_eq(
        w.balance(&w.council[0]).await?,
        proposer_before,
        NearToken::from_millinear(1),
    );
    assert_almost_eq(
        w.balance(&anyone).await?,
        anyone_before,
        NearToken::from_millinear(1),
    );

    Ok(())
}
