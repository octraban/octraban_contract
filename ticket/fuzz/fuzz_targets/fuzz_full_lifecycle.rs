//! Fuzz target: full lifecycle (mint → transfer → verify).
//!
//! Exercises the complete ticket state machine with random inputs.
//! The invariant checked: after any sequence of valid operations the
//! contract must not Rust-panic and the state must be self-consistent.

#![no_main]

use libfuzzer_sys::fuzz_target;
use soroban_sdk::{testutils::Address as _, Env, String};
use ticket::{TicketContractClient, TicketStatus};

fuzz_target!(|data: &[u8]| {
    if data.len() < 17 {
        return;
    }

    // Control byte: which path to exercise.
    let path = data[0] % 4;
    let sale_price = i128::from_le_bytes(data[1..17].try_into().unwrap()).abs();

    let env = Env::default();
    env.mock_all_auths();
    let id        = env.register_contract(None, ticket::TicketContract);
    let client    = TicketContractClient::new(&env, &id);
    let organizer = soroban_sdk::Address::generate(&env);
    let buyer     = soroban_sdk::Address::generate(&env);
    let new_owner = soroban_sdk::Address::generate(&env);

    client.initialize(
        &organizer,
        &String::from_str(&env, "Lifecycle Fuzz"),
        &5u64,
        &1_000i128,
        &100_000i128,
    );
    client.mint_ticket(&organizer, &buyer);

    match path {
        0 => {
            // Path A: mint → verify
            let ok = client.verify_ticket(&organizer, &0u64);
            assert!(ok);
            // ticket must now be Used
            assert_eq!(client.get_ticket(&0u64).status, TicketStatus::Used);
        }
        1 => {
            // Path B: mint → transfer (valid price)
            let capped = sale_price.min(100_000i128);
            let _ = client.try_transfer_ticket(&buyer, &new_owner, &0u64, &capped);
        }
        2 => {
            // Path C: mint → transfer (possibly invalid price) → verify
            let _ = client.try_transfer_ticket(&buyer, &new_owner, &0u64, &sale_price);
            // Attempt verify regardless of transfer outcome.
            let _ = client.try_verify_ticket(&organizer, &0u64);
        }
        _ => {
            // Path D: mint → verify → attempt transfer (must fail)
            client.verify_ticket(&organizer, &0u64);
            let result = client.try_transfer_ticket(&buyer, &new_owner, &0u64, &sale_price);
            assert!(result.is_err(), "transfer of used ticket must fail");
        }
    }
});
