//! Fuzz target: transfer_ticket with malformed/boundary prices.
//!
//! Feeds arbitrary bytes as sale_price and verifies the resale cap invariant:
//! if price > max_resale  → must error
//! if price <= max_resale → must succeed

#![no_main]

use libfuzzer_sys::fuzz_target;
use soroban_sdk::{testutils::Address as _, Env, String};
use ticket::TicketContractClient;

fuzz_target!(|data: &[u8]| {
    if data.len() < 16 {
        return;
    }

    let sale_price = i128::from_le_bytes(data[0..16].try_into().unwrap());
    // Avoid negative prices — the contract does not explicitly reject them
    // but they would always satisfy the cap, making the test trivial.
    if sale_price < 0 {
        return;
    }

    let max_resale: i128 = 75_000_000;

    let env = Env::default();
    env.mock_all_auths();
    let id        = env.register_contract(None, ticket::TicketContract);
    let client    = TicketContractClient::new(&env, &id);
    let organizer = soroban_sdk::Address::generate(&env);
    let buyer     = soroban_sdk::Address::generate(&env);
    let new_owner = soroban_sdk::Address::generate(&env);

    client.initialize(
        &organizer,
        &String::from_str(&env, "Fuzz Transfer"),
        &10u64,
        &1_000i128,
        &max_resale,
    );
    client.mint_ticket(&organizer, &buyer);

    let result = client.try_transfer_ticket(&buyer, &new_owner, &0u64, &sale_price);

    if sale_price > max_resale {
        assert!(result.is_err(), "transfer above cap must fail for price={sale_price}");
    } else {
        assert!(result.is_ok(), "transfer at/below cap must succeed for price={sale_price}");
    }
});
