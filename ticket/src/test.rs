//! Tests for the Ticket contract.
//!
//! Sections
//! --------
//! 1. Original unit tests (preserved)
//! 2. Property-based tests  (proptest)
//! 3. Snapshot / state-diff tests
//! 4. Gas benchmark tests
//! 5. Stress tests
//! 6. Edge-case / error-path tests

#![cfg(test)]

use super::*;
use soroban_sdk::{testutils::Address as _, Env, String};

// ─── helpers ─────────────────────────────────────────────────────────────────

/// Shared setup: deploy + initialise the contract with default parameters.
fn setup() -> (Env, TicketContractClient<'static>, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register_contract(None, TicketContract);
    let client = TicketContractClient::new(&env, &contract_id);

    let organizer = Address::generate(&env);
    let buyer = Address::generate(&env);

    client.initialize(
        &organizer,
        &String::from_str(&env, "Harvesta Live 2025"),
        &100u64,
        &50_000_000i128, // 5 XLM in stroops
        &75_000_000i128, // max resale 7.5 XLM
    );

    (env, client, organizer, buyer)
}

/// Setup with custom capacity.
fn setup_with_capacity(max: u64) -> (Env, TicketContractClient<'static>, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register_contract(None, TicketContract);
    let client = TicketContractClient::new(&env, &contract_id);
    let organizer = Address::generate(&env);
    client.initialize(
        &organizer,
        &String::from_str(&env, "Test Event"),
        &max,
        &1_000i128,
        &2_000i128,
    );
    (env, client, organizer)
}

// ═══════════════════════════════════════════════════════════════════════════
// 1. ORIGINAL UNIT TESTS
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn test_mint_and_get() {
    let (_env, client, organizer, buyer) = setup();
    let id = client.mint_ticket(&organizer, &buyer);
    assert_eq!(id, 0);

    let ticket = client.get_ticket(&0u64);
    assert_eq!(ticket.owner, buyer);
    assert_eq!(ticket.status, TicketStatus::Valid);
}

#[test]
fn test_transfer() {
    let (env, client, organizer, buyer) = setup();
    client.mint_ticket(&organizer, &buyer);

    let new_owner = Address::generate(&env);
    client.transfer_ticket(&buyer, &new_owner, &0u64, &60_000_000i128);

    let ticket = client.get_ticket(&0u64);
    assert_eq!(ticket.owner, new_owner);
    assert_eq!(ticket.status, TicketStatus::Transferred);
}

#[test]
#[should_panic(expected = "price exceeds resale cap")]
fn test_resale_cap_enforced() {
    let (env, client, organizer, buyer) = setup();
    client.mint_ticket(&organizer, &buyer);

    let new_owner = Address::generate(&env);
    client.transfer_ticket(&buyer, &new_owner, &0u64, &100_000_000i128);
}

#[test]
fn test_verify_ticket() {
    let (_env, client, organizer, buyer) = setup();
    client.mint_ticket(&organizer, &buyer);

    let valid = client.verify_ticket(&organizer, &0u64);
    assert!(valid);

    // Second scan must return false (already used).
    let double_scan = client.verify_ticket(&organizer, &0u64);
    assert!(!double_scan);
}

// ═══════════════════════════════════════════════════════════════════════════
// 2. PROPERTY-BASED TESTS (proptest)
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod property_tests {
    use super::*;
    use proptest::prelude::*;

    // --- helper that creates a fresh env for each proptest run --------------
    fn fresh_client(
        max_tickets: u64,
        price: i128,
        max_resale: i128,
    ) -> (Env, TicketContractClient<'static>, Address) {
        let env = Env::default();
        env.mock_all_auths();
        let id = env.register_contract(None, TicketContract);
        let client = TicketContractClient::new(&env, &id);
        let organizer = Address::generate(&env);
        client.initialize(
            &organizer,
            &String::from_str(&env, "Prop Event"),
            &max_tickets,
            &price,
            &max_resale,
        );
        (env, client, organizer)
    }

    // Property 1 ─ initialization always stores the admin address correctly.
    // We verify indirectly: only the admin can mint; anyone else panics.
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(30))]
        #[test]
        fn prop_initialize_admin_is_enforced(
            max_tickets in 1u64..500u64,
            price       in 1i128..1_000_000_000i128,
            max_resale  in 1i128..2_000_000_000i128,
        ) {
            let (env, client, organizer) = fresh_client(max_tickets, price, max_resale);
            let buyer    = Address::generate(&env);
            let impostor = Address::generate(&env);

            // Legitimate mint succeeds.
            let id = client.mint_ticket(&organizer, &buyer);
            let ticket = client.get_ticket(&id);
            prop_assert_eq!(ticket.owner, buyer);
            prop_assert_eq!(ticket.status, TicketStatus::Valid);

            // Impostor mint must fail.
            let result = client.try_mint_ticket(&impostor, &buyer);
            prop_assert!(result.is_err(), "non-admin should not be able to mint");
        }
    }

    // Property 2 ─ mint always assigns sequential IDs starting at 0.
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(20))]
        #[test]
        fn prop_sequential_ids(count in 1u64..50u64) {
            let (env, client, organizer) = fresh_client(count, 100, 200);
            for expected_id in 0..count {
                let buyer = Address::generate(&env);
                let id    = client.mint_ticket(&organizer, &buyer);
                prop_assert_eq!(id, expected_id, "ID must be sequential");
            }
            prop_assert_eq!(client.tickets_sold(), count);
        }
    }

    // Property 3 ─ resale price at exactly the cap always succeeds; one stroop
    //              over always fails.
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(30))]
        #[test]
        fn prop_resale_cap_boundary(max_resale in 1i128..500_000_000i128) {
            let (env, client, organizer) = fresh_client(10, 1, max_resale);
            let buyer     = Address::generate(&env);
            let new_owner = Address::generate(&env);
            client.mint_ticket(&organizer, &buyer);

            // At exactly the cap → must succeed.
            let ok = client.try_transfer_ticket(&buyer, &new_owner, &0u64, &max_resale);
            prop_assert!(ok.is_ok(), "transfer at cap should succeed");

            // One stroop over → must fail.
            let (env2, client2, org2) = fresh_client(10, 1, max_resale);
            let b2 = Address::generate(&env2);
            let n2 = Address::generate(&env2);
            client2.mint_ticket(&org2, &b2);
            let over = client2.try_transfer_ticket(&b2, &n2, &0u64, &(max_resale + 1));
            prop_assert!(over.is_err(), "transfer one-over cap should fail");
        }
    }

    // Property 4 ─ tickets_sold counter always equals the number of mints.
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(20))]
        #[test]
        fn prop_tickets_sold_counter(count in 0u64..30u64) {
            let (env, client, organizer) = fresh_client(count + 1, 100, 200);
            for _ in 0..count {
                let buyer = Address::generate(&env);
                client.mint_ticket(&organizer, &buyer);
            }
            prop_assert_eq!(client.tickets_sold(), count);
        }
    }

    // Property 5 ─ a transferred ticket cannot be transferred again.
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(20))]
        #[test]
        fn prop_double_transfer_fails(_dummy in 0u8..10u8) {
            let (env, client, organizer) = fresh_client(5, 100, 500);
            let buyer  = Address::generate(&env);
            let owner2 = Address::generate(&env);
            let owner3 = Address::generate(&env);
            client.mint_ticket(&organizer, &buyer);

            // First transfer.
            client.transfer_ticket(&buyer, &owner2, &0u64, &100i128);

            // Second transfer of the same ticket must fail.
            let result = client.try_transfer_ticket(&owner2, &owner3, &0u64, &100i128);
            prop_assert!(result.is_err(), "transferred ticket must not be re-transferred");
        }
    }

    // Property 6 ─ verified (used) ticket cannot be transferred.
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(20))]
        #[test]
        fn prop_used_ticket_not_transferable(_dummy in 0u8..10u8) {
            let (env, client, organizer) = fresh_client(5, 100, 500);
            let buyer     = Address::generate(&env);
            let new_owner = Address::generate(&env);
            client.mint_ticket(&organizer, &buyer);
            client.verify_ticket(&organizer, &0u64);

            let result = client.try_transfer_ticket(&buyer, &new_owner, &0u64, &100i128);
            prop_assert!(result.is_err(), "used ticket must not be transferable");
        }
    }

    // Property 7 ─ sale_price = 0 is always valid (free giveaway).
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(20))]
        #[test]
        fn prop_zero_price_transfer_always_valid(max_resale in 0i128..1_000_000i128) {
            let (env, client, organizer) = fresh_client(5, 0, max_resale);
            let buyer     = Address::generate(&env);
            let new_owner = Address::generate(&env);
            client.mint_ticket(&organizer, &buyer);
            let result = client.try_transfer_ticket(&buyer, &new_owner, &0u64, &0i128);
            prop_assert!(result.is_ok(), "zero-price transfer should always succeed");
        }
    }

    // Property 8 ─ minting beyond max_tickets always fails.
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(15))]
        #[test]
        fn prop_sold_out_rejects_mint(max in 1u64..20u64) {
            let (env, client, organizer) = fresh_client(max, 100, 200);
            for _ in 0..max {
                let buyer = Address::generate(&env);
                client.mint_ticket(&organizer, &buyer);
            }
            // One extra mint must fail.
            let overflow_buyer = Address::generate(&env);
            let result = client.try_mint_ticket(&organizer, &overflow_buyer);
            prop_assert!(result.is_err(), "mint beyond capacity must fail");
        }
    }

    // Property 9 ─ only the owner can transfer their own ticket.
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(20))]
        #[test]
        fn prop_only_owner_can_transfer(_dummy in 0u8..10u8) {
            let (env, client, organizer) = fresh_client(5, 100, 500);
            let real_owner = Address::generate(&env);
            let thief      = Address::generate(&env);
            let target     = Address::generate(&env);
            client.mint_ticket(&organizer, &real_owner);

            let result = client.try_transfer_ticket(&thief, &target, &0u64, &100i128);
            prop_assert!(result.is_err(), "non-owner must not transfer ticket");
        }
    }

    // Property 10 ─ only organizer can verify tickets.
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(20))]
        #[test]
        fn prop_only_organizer_can_verify(_dummy in 0u8..10u8) {
            let (env, client, organizer) = fresh_client(5, 100, 500);
            let buyer    = Address::generate(&env);
            let impostor = Address::generate(&env);
            client.mint_ticket(&organizer, &buyer);

            let result = client.try_verify_ticket(&impostor, &0u64);
            prop_assert!(result.is_err(), "non-organizer must not verify ticket");
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// 3. SNAPSHOT / STATE-DIFF TESTS
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod snapshot_tests {
    use super::*;

    /// After initialization the counter is zero and no tickets exist.
    #[test]
    fn snapshot_after_initialize() {
        let (_env, client, _organizer, _buyer) = setup();
        assert_eq!(client.tickets_sold(), 0, "snapshot: counter should be 0 post-init");
    }

    /// After one mint: counter == 1, ticket is Valid, owner matches.
    #[test]
    fn snapshot_after_mint() {
        let (_env, client, organizer, buyer) = setup();
        client.mint_ticket(&organizer, &buyer);

        assert_eq!(client.tickets_sold(), 1);
        let t = client.get_ticket(&0u64);
        assert_eq!(t.id, 0);
        assert_eq!(t.owner, buyer);
        assert_eq!(t.status, TicketStatus::Valid);
        assert_eq!(t.original_price, 50_000_000i128);
        assert_eq!(t.max_resale_price, 75_000_000i128);
    }

    /// After transfer: status is Transferred, owner changed, price unchanged.
    #[test]
    fn snapshot_after_transfer() {
        let (env, client, organizer, buyer) = setup();
        client.mint_ticket(&organizer, &buyer);
        let new_owner = Address::generate(&env);
        client.transfer_ticket(&buyer, &new_owner, &0u64, &60_000_000i128);

        let t = client.get_ticket(&0u64);
        assert_eq!(t.owner, new_owner);
        assert_eq!(t.status, TicketStatus::Transferred);
        // original_price must not mutate.
        assert_eq!(t.original_price, 50_000_000i128);
    }

    /// After verify: status is Used, ownership unchanged.
    #[test]
    fn snapshot_after_verify() {
        let (_env, client, organizer, buyer) = setup();
        client.mint_ticket(&organizer, &buyer);
        client.verify_ticket(&organizer, &0u64);

        let t = client.get_ticket(&0u64);
        assert_eq!(t.status, TicketStatus::Used);
        assert_eq!(t.owner, buyer, "owner must not change on verify");
    }

    /// Full lifecycle: mint → transfer → verify. Each state transition
    /// produces the expected snapshot.
    #[test]
    fn snapshot_full_lifecycle() {
        let (env, client, organizer, buyer) = setup();
        client.mint_ticket(&organizer, &buyer);

        // After mint.
        assert_eq!(client.get_ticket(&0u64).status, TicketStatus::Valid);

        // After transfer.
        let new_owner = Address::generate(&env);
        client.transfer_ticket(&buyer, &new_owner, &0u64, &70_000_000i128);
        assert_eq!(client.get_ticket(&0u64).status, TicketStatus::Transferred);
        assert_eq!(client.get_ticket(&0u64).owner, new_owner);

        // After verify.
        client.verify_ticket(&organizer, &0u64);
        assert_eq!(client.get_ticket(&0u64).status, TicketStatus::Used);

        // Counter must still be 1 (verify does not burn the ticket).
        assert_eq!(client.tickets_sold(), 1);
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// 4. GAS BENCHMARK TESTS
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod gas_benchmark_tests {
    use super::*;

    /// Budget ceiling (in Soroban CPU instructions).
    /// Adjust as the contract evolves; failing here flags a regression.
    const INITIALIZE_CPU_LIMIT: u64 = 100_000_000;
    const MINT_CPU_LIMIT: u64       = 100_000_000;
    const TRANSFER_CPU_LIMIT: u64   = 100_000_000;
    const VERIFY_CPU_LIMIT: u64     = 100_000_000;

    fn env_with_budget() -> Env {
        let env = Env::default();
        env.mock_all_auths();
        // Reset budget tracking before each measured call.
        env.budget().reset_default();
        env
    }

    #[test]
    fn bench_initialize_gas() {
        let env = env_with_budget();
        let id = env.register_contract(None, TicketContract);
        let client = TicketContractClient::new(&env, &id);
        let organizer = Address::generate(&env);

        env.budget().reset_default();
        client.initialize(
            &organizer,
            &String::from_str(&env, "Bench Event"),
            &1000u64,
            &50_000_000i128,
            &75_000_000i128,
        );
        let cpu = env.budget().cpu_instruction_count();
        assert!(
            cpu <= INITIALIZE_CPU_LIMIT,
            "initialize() used {cpu} CPU instructions (limit: {INITIALIZE_CPU_LIMIT})"
        );
    }

    #[test]
    fn bench_mint_gas_minimum_input() {
        let (env, client, organizer, buyer) = setup();
        env.budget().reset_default();
        client.mint_ticket(&organizer, &buyer);
        let cpu = env.budget().cpu_instruction_count();
        assert!(
            cpu <= MINT_CPU_LIMIT,
            "mint_ticket() used {cpu} CPU instructions (limit: {MINT_CPU_LIMIT})"
        );
    }

    #[test]
    fn bench_transfer_gas() {
        let (env, client, organizer, buyer) = setup();
        client.mint_ticket(&organizer, &buyer);
        let new_owner = Address::generate(&env);

        env.budget().reset_default();
        client.transfer_ticket(&buyer, &new_owner, &0u64, &60_000_000i128);
        let cpu = env.budget().cpu_instruction_count();
        assert!(
            cpu <= TRANSFER_CPU_LIMIT,
            "transfer_ticket() used {cpu} CPU instructions (limit: {TRANSFER_CPU_LIMIT})"
        );
    }

    #[test]
    fn bench_verify_gas() {
        let (_env, client, organizer, buyer) = setup();
        client.mint_ticket(&organizer, &buyer);

        _env.budget().reset_default();
        client.verify_ticket(&organizer, &0u64);
        let cpu = _env.budget().cpu_instruction_count();
        assert!(
            cpu <= VERIFY_CPU_LIMIT,
            "verify_ticket() used {cpu} CPU instructions (limit: {VERIFY_CPU_LIMIT})"
        );
    }

    /// Gas must not scale super-linearly — verify that the 100th mint costs
    /// no more than 2× the first mint.
    #[test]
    fn bench_mint_gas_does_not_degrade() {
        let (env, client, organizer, _) = setup_with_capacity(200);

        // Warm up — mint 99 tickets.
        for _ in 0..99 {
            let b = Address::generate(&env);
            client.mint_ticket(&organizer, &b);
        }

        // Measure the 100th mint.
        let buyer = Address::generate(&env);
        env.budget().reset_default();
        client.mint_ticket(&organizer, &buyer);
        let cpu_100 = env.budget().cpu_instruction_count();

        // Measure the first mint of a fresh contract.
        let (env2, client2, org2) = setup_with_capacity(200);
        let b2 = Address::generate(&env2);
        env2.budget().reset_default();
        client2.mint_ticket(&org2, &b2);
        let cpu_1 = env2.budget().cpu_instruction_count();

        assert!(
            cpu_100 <= cpu_1 * 2,
            "mint gas degraded: 1st={cpu_1}, 100th={cpu_100}"
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// 5. STRESS TESTS
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod stress_tests {
    use super::*;

    /// Mint 1 000 tickets and verify the counter + last ticket are correct.
    #[test]
    fn stress_mint_1000_tickets() {
        let n: u64 = 1_000;
        let (env, client, organizer) = setup_with_capacity(n);
        let mut last_buyer = Address::generate(&env);

        for i in 0..n {
            last_buyer = Address::generate(&env);
            let id = client.mint_ticket(&organizer, &last_buyer);
            assert_eq!(id, i);
        }

        assert_eq!(client.tickets_sold(), n);
        let last = client.get_ticket(&(n - 1));
        assert_eq!(last.owner, last_buyer);
        assert_eq!(last.status, TicketStatus::Valid);
    }

    /// Transfer 200 tickets sequentially and verify all end up as Transferred.
    #[test]
    fn stress_sequential_transfers() {
        let n: u64 = 200;
        let (env, client, organizer) = setup_with_capacity(n);

        let mut buyers: Vec<Address> = Vec::new();
        for _ in 0..n {
            let b = Address::generate(&env);
            client.mint_ticket(&organizer, &b);
            buyers.push(b);
        }

        for i in 0..n {
            let new_owner = Address::generate(&env);
            client.transfer_ticket(&buyers[i as usize], &new_owner, &i, &500i128);
            let t = client.get_ticket(&i);
            assert_eq!(t.status, TicketStatus::Transferred);
        }
    }

    /// Verify 500 tickets and confirm all are marked Used.
    #[test]
    fn stress_batch_verify_500() {
        let n: u64 = 500;
        let (env, client, organizer) = setup_with_capacity(n);

        for _ in 0..n {
            let b = Address::generate(&env);
            client.mint_ticket(&organizer, &b);
        }

        for i in 0..n {
            let result = client.verify_ticket(&organizer, &i);
            assert!(result, "ticket {i} should verify as true");
        }

        for i in 0..n {
            let t = client.get_ticket(&i);
            assert_eq!(t.status, TicketStatus::Used, "ticket {i} should be Used");
        }
    }

    /// Sold-out boundary: capacity = 1 → second mint must fail.
    #[test]
    fn stress_capacity_one_sold_out() {
        let (env, client, organizer) = setup_with_capacity(1);
        let b1 = Address::generate(&env);
        let b2 = Address::generate(&env);

        client.mint_ticket(&organizer, &b1);
        let result = client.try_mint_ticket(&organizer, &b2);
        assert!(result.is_err(), "second mint on capacity-1 contract must fail");
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// 6. EDGE-CASE / ERROR-PATH TESTS
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod error_path_tests {
    use super::*;

    /// Double-initialization must panic.
    #[test]
    fn error_double_initialize() {
        let (_env, client, organizer, _buyer) = setup();
        let result = client.try_initialize(
            &organizer,
            &String::from_str(&_env, "Dup Event"),
            &50u64,
            &1_000i128,
            &2_000i128,
        );
        assert!(result.is_err(), "second initialize must fail");
    }

    /// Getting a nonexistent ticket must panic.
    #[test]
    fn error_get_nonexistent_ticket() {
        let (_env, client, _organizer, _buyer) = setup();
        let result = client.try_get_ticket(&9999u64);
        assert!(result.is_err(), "fetching nonexistent ticket must fail");
    }

    /// Verify a nonexistent ticket must panic.
    #[test]
    fn error_verify_nonexistent_ticket() {
        let (_env, client, organizer, _buyer) = setup();
        let result = client.try_verify_ticket(&organizer, &9999u64);
        assert!(result.is_err(), "verifying nonexistent ticket must fail");
    }

    /// Transfer a nonexistent ticket must panic.
    #[test]
    fn error_transfer_nonexistent_ticket() {
        let (env, client, _organizer, buyer) = setup();
        let new_owner = Address::generate(&env);
        let result = client.try_transfer_ticket(&buyer, &new_owner, &9999u64, &0i128);
        assert!(result.is_err(), "transferring nonexistent ticket must fail");
    }

    /// Sale price exactly at max_resale must succeed (boundary value).
    #[test]
    fn error_transfer_at_exact_cap_succeeds() {
        let (env, client, organizer, buyer) = setup();
        client.mint_ticket(&organizer, &buyer);
        let new_owner = Address::generate(&env);
        // 75_000_000 is exactly the cap.
        let result = client.try_transfer_ticket(&buyer, &new_owner, &0u64, &75_000_000i128);
        assert!(result.is_ok(), "transfer at exact cap must succeed");
    }

    /// Non-organizer cannot call mint.
    #[test]
    fn error_non_organizer_cannot_mint() {
        let (env, client, _organizer, buyer) = setup();
        let impostor = Address::generate(&env);
        let result = client.try_mint_ticket(&impostor, &buyer);
        assert!(result.is_err());
    }

    /// Non-organizer cannot verify.
    #[test]
    fn error_non_organizer_cannot_verify() {
        let (env, client, organizer, buyer) = setup();
        client.mint_ticket(&organizer, &buyer);
        let impostor = Address::generate(&env);
        let result = client.try_verify_ticket(&impostor, &0u64);
        assert!(result.is_err());
    }

    /// Double-verify: first returns true, second returns false.
    #[test]
    fn error_double_verify_returns_false() {
        let (_env, client, organizer, buyer) = setup();
        client.mint_ticket(&organizer, &buyer);
        assert!(client.verify_ticket(&organizer, &0u64));
        assert!(!client.verify_ticket(&organizer, &0u64));
    }

    /// Verifying a transferred ticket must also succeed (and mark it Used).
    #[test]
    fn verify_transferred_ticket_succeeds() {
        let (env, client, organizer, buyer) = setup();
        client.mint_ticket(&organizer, &buyer);
        let new_owner = Address::generate(&env);
        client.transfer_ticket(&buyer, &new_owner, &0u64, &60_000_000i128);

        let result = client.verify_ticket(&organizer, &0u64);
        assert!(result, "verifying a transferred ticket must return true");
        assert_eq!(client.get_ticket(&0u64).status, TicketStatus::Used);
    }
}
