//! Integration tests of a real SputnikDAO v2 (2-of-3 council) driving the
//! dao-timelock, with a DAO policy that forces every policy upgrade through
//! the timelock. See `setup/sputnik_helpers.rs` for the topology.

mod setup;

use crate::setup::sputnik_helpers::{
    PROPOSAL_BOND, change_policy_kind, dao_policy, schedule_policy_change_kind,
    setup_sputnik_workspace,
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
    let outer_kind = schedule_policy_change_kind(w.timelock.id(), &new_policy, PROPOSAL_BOND);
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

    // The request cannot be executed before the delay and the policy is unchanged.
    let outcome = w.execute_request(&w.council[0], 0).await?;
    assert!(
        outcome.is_failure(),
        "Executing before the delay should fail: {:#?}",
        outcome.outcomes()
    );
    assert_eq!(w.get_policy().await?["proposal_bond"], json!(PROPOSAL_BOND));

    // After the delay, anyone executes: the timelock adds the ChangePolicy
    // proposal on the DAO and approves it with its single-member role vote,
    // using the proposal id returned by add_proposal.
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
async fn test_policy_cannot_be_changed_without_timelock() -> Result<(), Box<dyn std::error::Error>>
{
    let w = setup_sputnik_workspace().await?;
    let new_policy = dao_policy(&w.council, w.timelock.id(), NearToken::from_near(2));
    let kind = change_policy_kind(&new_policy);

    // No council member can add a ChangePolicy proposal directly: the council
    // role has no "policy:AddProposal" permission.
    for member in &w.council {
        let outcome = w.add_proposal_raw(member, &kind, PROPOSAL_BOND).await?;
        assert!(
            outcome.is_failure(),
            "A council member should not be able to add a policy proposal: {:#?}",
            outcome.outcomes()
        );
    }
    // Neither can an outsider or the timelock's guardian.
    let outsider = w.sandbox.dev_create_account().await?;
    for account in [&outsider, &w.guardian] {
        let outcome = w.add_proposal_raw(account, &kind, PROPOSAL_BOND).await?;
        assert!(
            outcome.is_failure(),
            "A non-member should not be able to add any proposal: {:#?}",
            outcome.outcomes()
        );
    }

    // Nobody but the DAO can schedule timelock requests directly, so the
    // council cannot bypass the DAO vote either.
    for account in [&w.council[0], &w.guardian] {
        let outcome = account
            .call(w.timelock.id(), "schedule")
            .args_json(json!({
                "receiver_id": w.dao.id(),
                "actions": [crate::setup::timelock_helpers::function_call(
                    "act_proposal",
                    json!({}),
                    NearToken::from_yoctonear(0),
                    10,
                )],
            }))
            .transact()
            .await?;
        assert!(
            outcome.is_failure(),
            "Only the DAO should be able to schedule: {:#?}",
            outcome.outcomes()
        );
    }

    assert_eq!(w.get_last_proposal_id().await?, 0);
    assert_eq!(w.get_num_requests().await?, 0);
    assert_eq!(w.get_policy().await?["proposal_bond"], json!(PROPOSAL_BOND));

    Ok(())
}

#[tokio::test]
async fn test_two_rejections_block_the_policy_change() -> Result<(), Box<dyn std::error::Error>> {
    let w = setup_sputnik_workspace().await?;
    let new_policy = dao_policy(&w.council, w.timelock.id(), NearToken::from_near(2));

    let outer_kind = schedule_policy_change_kind(w.timelock.id(), &new_policy, PROPOSAL_BOND);
    let proposal_id = w
        .add_proposal(&w.council[0], &outer_kind, PROPOSAL_BOND)
        .await?;

    // 1 approval vs 2 rejections: the proposal is rejected, nothing is scheduled.
    let outcome = w
        .act_proposal(&w.council[0], proposal_id, "VoteApprove", &outer_kind)
        .await?;
    outcome_check(&outcome);
    for voter in &w.council[1..] {
        let outcome = w
            .act_proposal(voter, proposal_id, "VoteReject", &outer_kind)
            .await?;
        outcome_check(&outcome);
    }

    assert_eq!(w.get_proposal_status(proposal_id).await?, "Rejected");
    assert_eq!(w.get_num_requests().await?, 0);
    assert_eq!(w.get_policy().await?["proposal_bond"], json!(PROPOSAL_BOND));

    Ok(())
}

#[tokio::test]
async fn test_guardian_cancels_scheduled_policy_change() -> Result<(), Box<dyn std::error::Error>> {
    let w = setup_sputnik_workspace().await?;
    let new_policy = dao_policy(&w.council, w.timelock.id(), NearToken::from_near(2));

    let dao_balance_before = w.balance(&w.dao).await?;
    let request_id = w.schedule_policy_change_via_council(&new_policy).await?;
    let last_proposal_id = w.get_last_proposal_id().await?;

    // The DAO escrowed the inner proposal bond in the timelock.
    assert_almost_eq(
        w.balance(&w.dao).await?,
        dao_balance_before.saturating_sub(PROPOSAL_BOND),
        NearToken::from_millinear(50),
    );

    // The guardian cancels the pending request during the delay; the escrowed
    // bond is refunded to the DAO.
    let outcome = w.cancel_request(&w.guardian, request_id).await?;
    outcome_check(&outcome);
    assert_eq!(w.get_num_requests().await?, 0);
    assert_almost_eq(
        w.balance(&w.dao).await?,
        dao_balance_before,
        NearToken::from_millinear(50),
    );

    // Even after the delay the cancelled request cannot be executed: the
    // policy is unchanged and the inner proposal was never created.
    w.fast_forward_to(w.sandbox.view_block().await?.timestamp() + 61 * 1_000_000_000)
        .await?;
    let outcome = w.execute_request(&w.guardian, request_id).await?;
    assert!(
        outcome.is_failure(),
        "Executing a cancelled request should fail: {:#?}",
        outcome.outcomes()
    );
    assert_eq!(w.get_policy().await?["proposal_bond"], json!(PROPOSAL_BOND));
    assert_eq!(w.get_last_proposal_id().await?, last_proposal_id);

    Ok(())
}

#[tokio::test]
async fn test_policy_change_survives_interleaved_proposals()
-> Result<(), Box<dyn std::error::Error>> {
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

    // The council schedules an inner proposal the timelock is not allowed to
    // submit: its role only has policy proposal permissions, not FunctionCall.
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
    let outcome = w.execute_request(&anyone, request_id).await?;

    // add_proposal fails, and only it: the callback refunds the bond instead
    // of voting.
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

    // No proposal was created, the request is consumed and the escrowed bond
    // went back to the DAO.
    assert_eq!(w.get_last_proposal_id().await?, last_proposal_id);
    assert_eq!(w.get_num_requests().await?, 0);
    assert_almost_eq(
        w.balance(&w.dao).await?,
        dao_balance_before,
        NearToken::from_millinear(50),
    );

    Ok(())
}
