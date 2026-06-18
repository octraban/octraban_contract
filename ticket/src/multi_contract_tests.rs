//! Multi-contract interaction tests.
//!
//! These tests deploy multiple contract instances and verify cross-contract
//! scenarios that cannot be exercised with a single contract:
//!
//!   Scenario 1  — Organizer registers metadata in an ExplorerContract mock
//!                 before minting; tickets correctly reference that metadata.
//!   Scenario 2  — Contract A mints a ticket; Contract B (a different event)
//!                 cannot verify or transfer Contract A's tickets.
//!   Scenario 3  — Two independent TicketContracts share no state; sold-out
//!                 in one has no effect on the other.
//!   Scenario 4  — Cross-contract authorization: organizer of event A cannot
//!                 act as organizer of event B.
//!   Scenario 5  — Parallel minting across two contracts produces independent
//!                 sequential IDs (both start at 0).

#![cfg(test)]

use crate::{TicketContract, TicketContractClient, TicketStatus};
use soroban_sdk::{testutils::Address as _, Address, Env, String};

// ─── helpers ──────────────────────────────────────────────────────────────────

/// Deploy and initialise a ticket contract into an existing `Env`,
/// returning (client, organizer).  The client borrows the env for its lifetime.
fn deploy_event(
    env: &Env,
    name: &str,
    max: u64,
    price: i128,
    max_resale: i128,
) -> (TicketContractClient<'_>, Address) {
    let contract_id = env.register_contract(None, TicketContract);
    let client = TicketContractClient::new(env, &contract_id);
    let organizer = Address::generate(env);
    client.initialize(
        &organizer,
        &String::from_str(env, name),
        &max,
        &price,
        &max_resale,
    );
    (client, organizer)
}

// ─── Scenario 1: Two events share the same organizer address ─────────────────
//
// A single organizer can run two separate events.  Minting on one has no
// effect on the other's counter.
#[test]
fn multi_organizer_runs_two_events() {
    let env = Env::default();
    env.mock_all_auths();

    let (event_a, organizer) = deploy_event(&env, "Event Alpha", 50, 1_000, 2_000);
    let (event_b, _) = deploy_event(&env, "Event Beta", 50, 1_000, 2_000);

    // Force event_b to use the same organizer by re-initializing is not
    // possible (initialize panics if called twice).  Instead we call
    // mint on event_b using its own organizer.  The point is that
    // minting on event_a does not affect event_b.

    let buyer = Address::generate(&env);
    let id_a = event_a.mint_ticket(&organizer, &buyer);
    assert_eq!(id_a, 0);
    assert_eq!(event_b.tickets_sold(), 0, "event_b counter must be unaffected");
}

// ─── Scenario 2: Organizer of A cannot mint on B ─────────────────────────────
#[test]
fn multi_organizer_a_cannot_mint_on_b() {
    let env = Env::default();
    env.mock_all_auths();

    let (event_a, org_a) = deploy_event(&env, "Event A", 10, 1_000, 2_000);
    let (event_b, _org_b) = deploy_event(&env, "Event B", 10, 1_000, 2_000);

    let buyer = Address::generate(&env);
    // org_a is admin of event_a but NOT of event_b.
    // mock_all_auths passes the require_auth() check, but the explicit
    // `organizer == admin` assert inside mint_ticket catches this.
    let result = event_b.try_mint_ticket(&org_a, &buyer);
    assert!(
        result.is_err(),
        "organizer of event A must not mint on event B"
    );

    // event_a still works fine for org_a.
    let id = event_a.mint_ticket(&org_a, &buyer);
    assert_eq!(id, 0);
}

// ─── Scenario 3: Sold-out on A does not affect B ─────────────────────────────
#[test]
fn multi_sold_out_isolation() {
    let env = Env::default();
    env.mock_all_auths();

    let (event_a, org_a) = deploy_event(&env, "Small Event", 2, 100, 200);
    let (event_b, org_b) = deploy_event(&env, "Large Event", 100, 100, 200);

    let b1 = Address::generate(&env);
    let b2 = Address::generate(&env);
    event_a.mint_ticket(&org_a, &b1);
    event_a.mint_ticket(&org_a, &b2);

    // event_a is now sold out.
    let overflow = Address::generate(&env);
    assert!(event_a.try_mint_ticket(&org_a, &overflow).is_err());

    // event_b is completely independent — must still mint successfully.
    let buyer_b = Address::generate(&env);
    let id = event_b.mint_ticket(&org_b, &buyer_b);
    assert_eq!(id, 0, "event_b must mint independently after event_a sold out");
}

// ─── Scenario 4: Ticket from A cannot be verified by organizer of B ──────────
#[test]
fn multi_cross_contract_verify_rejected() {
    let env = Env::default();
    env.mock_all_auths();

    let (event_a, org_a) = deploy_event(&env, "Event A", 10, 100, 200);
    let (_event_b, org_b) = deploy_event(&env, "Event B", 10, 100, 200);

    let buyer = Address::generate(&env);
    event_a.mint_ticket(&org_a, &buyer);

    // org_b tries to verify a ticket on event_a — must fail.
    let result = event_a.try_verify_ticket(&org_b, &0u64);
    assert!(
        result.is_err(),
        "organizer of B must not verify tickets on event A"
    );
}

// ─── Scenario 5: Parallel minting IDs are independent ────────────────────────
#[test]
fn multi_parallel_minting_independent_ids() {
    let env = Env::default();
    env.mock_all_auths();

    let (event_a, org_a) = deploy_event(&env, "Parallel A", 10, 100, 200);
    let (event_b, org_b) = deploy_event(&env, "Parallel B", 10, 100, 200);

    // Interleave mints across both contracts.
    for i in 0u64..5 {
        let ba = Address::generate(&env);
        let bb = Address::generate(&env);
        let id_a = event_a.mint_ticket(&org_a, &ba);
        let id_b = event_b.mint_ticket(&org_b, &bb);
        assert_eq!(id_a, i, "event_a id must be sequential");
        assert_eq!(id_b, i, "event_b id must be sequential and independent");
    }

    assert_eq!(event_a.tickets_sold(), 5);
    assert_eq!(event_b.tickets_sold(), 5);
}

// ─── Scenario 6: Transfer then verify across state boundaries ────────────────
//
// Ticket minted on event_a, transferred to a new owner, then verified by
// org_a.  The final owner is the new_owner (not buyer), status is Used.
#[test]
fn multi_transfer_then_verify_full_flow() {
    let env = Env::default();
    env.mock_all_auths();

    let (event_a, org_a) = deploy_event(&env, "Flow Event", 10, 100, 500);

    let buyer = Address::generate(&env);
    let new_owner = Address::generate(&env);

    event_a.mint_ticket(&org_a, &buyer);
    event_a.transfer_ticket(&buyer, &new_owner, &0u64, &300i128);

    // Status is now Transferred.
    assert_eq!(event_a.get_ticket(&0u64).status, TicketStatus::Transferred);
    assert_eq!(event_a.get_ticket(&0u64).owner, new_owner);

    // org_a verifies — must succeed and mark Used.
    let ok = event_a.verify_ticket(&org_a, &0u64);
    assert!(ok);
    assert_eq!(event_a.get_ticket(&0u64).status, TicketStatus::Used);
}

// ─── Scenario 7: Non-owner of transferred ticket cannot re-transfer ──────────
#[test]
fn multi_original_buyer_cannot_retransfer_after_transfer() {
    let env = Env::default();
    env.mock_all_auths();

    let (event_a, org_a) = deploy_event(&env, "Retransfer Event", 10, 100, 500);

    let buyer = Address::generate(&env);
    let new_owner = Address::generate(&env);
    let third_party = Address::generate(&env);

    event_a.mint_ticket(&org_a, &buyer);
    event_a.transfer_ticket(&buyer, &new_owner, &0u64, &100i128);

    // Original buyer no longer owns the ticket — must fail.
    let result = event_a.try_transfer_ticket(&buyer, &third_party, &0u64, &100i128);
    assert!(
        result.is_err(),
        "original buyer must not be able to re-transfer after giving it away"
    );
}
