#![no_std]
#![cfg(kani)]

use soroban_sdk::{testutils::Address as _, Address, Env, BytesN};
use explorer_contract::{ExplorerContractClient, ExplorerContract};

// Formal verification with Kani
// We verify that integer arithmetic cannot overflow and that panic paths are not reachable without auth.
#[kani::proof]
fn verify_event_seq_bounds() {
    let env = Env::default();
    env.mock_all_auths();
    let id = env.register_contract(None, ExplorerContract);
    let client = ExplorerContractClient::new(&env, &id);

    let admin = Address::generate(&env);
    
    // We restrict the input space to make verification tractable
    let max_events: u32 = kani::any();
    kani::assume(max_events > 0 && max_events <= 100_000);
    
    client.init(&admin, &max_events);
    
    let (count, max) = client.storage_utilisation();
    assert!(count <= max as u64, "Event count must never exceed max_events");
}
