use soroban_sdk::{testutils::Address as _, Address, Env, String, Vec, BytesN};
use proptest::prelude::*;
use explorer_contract::{ExplorerContractClient, ContractMeta, ExplorerContract, MIN_MAX_EVENTS, DEFAULT_MAX_EVENTS};

proptest! {
    #[test]
    fn test_init_invariants(max_events in 0u32..u32::MAX) {
        let env = Env::default();
        env.mock_all_auths();
        let id = env.register_contract(None, ExplorerContract);
        let client = ExplorerContractClient::new(&env, &id);
        
        let admin = Address::generate(&env);
        client.init(&admin, &max_events);
        
        let (_, stored_max) = client.storage_utilisation();
        
        if max_events == 0 {
            prop_assert_eq!(stored_max, DEFAULT_MAX_EVENTS);
        } else {
            prop_assert_eq!(stored_max, max_events);
        }
    }
}
