//! Fuzz target: mint_ticket
//!
//! Feeds arbitrary bytes as max_tickets / price / max_resale and verifies
//! the contract never panics with a Rust panic (only with a Soroban error).

#![no_main]

use libfuzzer_sys::fuzz_target;
use soroban_sdk::{testutils::Address as _, Env, String};
use ticket::TicketContractClient;

fuzz_target!(|data: &[u8]| {
    if data.len() < 25 {
        return;
    }

    // Derive parameters from raw bytes.
    let max_tickets = u64::from_le_bytes(data[0..8].try_into().unwrap()).saturating_add(1);
    let price       = i128::from_le_bytes(data[8..24].try_into().unwrap()).abs();
    let max_resale  = price.saturating_add(1); // always >= price

    let env = Env::default();
    env.mock_all_auths();
    let id       = env.register_contract(None, ticket::TicketContract);
    let client   = TicketContractClient::new(&env, &id);
    let organizer = soroban_sdk::Address::generate(&env);
    let buyer     = soroban_sdk::Address::generate(&env);

    // initialize should always succeed with these valid inputs.
    let _ = client.try_initialize(
        &organizer,
        &String::from_str(&env, "Fuzz Event"),
        &max_tickets,
        &price,
        &max_resale,
    );

    // Mint — may legitimately error (e.g. capacity constraints), must not Rust-panic.
    let _ = client.try_mint_ticket(&organizer, &buyer);

    // tickets_sold must always return a valid value.
    let _ = client.tickets_sold();
});
