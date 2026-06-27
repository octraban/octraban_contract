#![no_main]

use libfuzzer_sys::fuzz_target;
use soroban_sdk::{testutils::Address as _, Address, Env, BytesN, String, Vec, Bytes};
use explorer_contract::{ExplorerContractClient, ExplorerContract, EventInput};

fuzz_target!(|data: &[u8]| {
    // We require enough data to generate some primitive fields
    if data.len() < 32 {
        return;
    }

    let env = Env::default();
    env.mock_all_auths();
    let id = env.register_contract(None, ExplorerContract);
    let client = ExplorerContractClient::new(&env, &id);

    let admin = Address::generate(&env);
    client.init(&admin, &5000);

    let mut cid_array = [0u8; 32];
    cid_array.copy_from_slice(&data[0..32]);
    let cid = BytesN::from_array(&env, &cid_array);

    let input = EventInput {
        contract_id: cid,
        function: soroban_sdk::symbol_short!("swap"),
        ledger: 100,
        description: String::from_str(&env, "Fuzzed Event"),
        raw_topics: Vec::new(&env),
        raw_data: Bytes::new(&env),
    };

    // The core invariants are that submission doesn't panic unless auth fails
    client.submit_event(&admin, &input);
});
