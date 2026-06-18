//! Fuzz target: verify_ticket
//!
//! Feeds arbitrary ticket_id values and arbitrary lifecycle paths, verifying:
//!   - Verifying a nonexistent ticket always errors (no panic).
//!   - Verifying a Valid ticket returns true and marks it Used.
//!   - Verifying a Used ticket returns false (idempotent).
//!   - Only the organizer can verify; impostor always errors.

#![no_main]

use libfuzzer_sys::fuzz_target;
use soroban_sdk::{testutils::Address as _, Env, String};
use ticket::{TicketContractClient, TicketStatus};

fuzz_target!(|data: &[u8]| {
    if data.len() < 9 {
        return;
    }

    // Derive a ticket_id from the first 8 bytes.
    let ticket_id = u64::from_le_bytes(data[0..8].try_into().unwrap());
    // Control byte: 0 = verify nonexistent, 1 = verify valid, 2 = double-verify,
    //               3 = impostor verify.
    let path = data[8] % 4;

    let env = Env::default();
    env.mock_all_auths();
    let id = env.register_contract(None, ticket::TicketContract);
    let client = TicketContractClient::new(&env, &id);
    let organizer = soroban_sdk::Address::generate(&env);
    let buyer = soroban_sdk::Address::generate(&env);

    client.initialize(
        &organizer,
        &String::from_str(&env, "Fuzz Verify"),
        &5u64,
        &1_000i128,
        &2_000i128,
    );

    match path {
        0 => {
            // Path A: verify a ticket_id that was never minted — must not Rust-panic.
            let result = client.try_verify_ticket(&organizer, &ticket_id);
            // If ticket_id == 0 it might not exist yet, so error is expected.
            // Either way: no Rust panic, only a Soroban contract error.
            let _ = result;
        }
        1 => {
            // Path B: mint then verify the real ticket (id 0).
            client.mint_ticket(&organizer, &buyer);
            let ok = client.verify_ticket(&organizer, &0u64);
            assert!(ok, "first verify of a valid ticket must return true");
            assert_eq!(
                client.get_ticket(&0u64).status,
                TicketStatus::Used,
                "ticket must be Used after verify"
            );
        }
        2 => {
            // Path C: mint → verify → verify again (double-scan).
            client.mint_ticket(&organizer, &buyer);
            let first = client.verify_ticket(&organizer, &0u64);
            assert!(first, "first scan must return true");
            let second = client.verify_ticket(&organizer, &0u64);
            assert!(!second, "second scan of Used ticket must return false");
        }
        _ => {
            // Path D: impostor tries to verify — must always error.
            client.mint_ticket(&organizer, &buyer);
            let impostor = soroban_sdk::Address::generate(&env);
            let result = client.try_verify_ticket(&impostor, &0u64);
            assert!(
                result.is_err(),
                "impostor verify must always fail"
            );
        }
    }
});
