#![no_std]
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, panic_with_error, symbol_short, Address,
    Bytes, BytesN, Env, String, Symbol, Vec,
};

// ── Error codes ──────────────────────────────────────────────────────────────
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    NotFound = 1,
    Unauthorized = 2,
    AlreadyExists = 3,
}

// ── Storage keys ─────────────────────────────────────────────────────────────
#[contracttype]
pub enum DataKey {
    Admin,
    Contract(BytesN<32>), // contract_id → ContractMeta
    EventLog(u64),        // seq → DecodedEvent
    EventSeq,
}

// ── Data types ────────────────────────────────────────────────────────────────

/// ABI-like metadata for a registered contract.
#[contracttype]
#[derive(Clone)]
pub struct ContractMeta {
    pub version: u32, // Metadata schema version for forward compatibility
    pub name: String, // e.g. "StellarSwap"
    pub description: String,
    pub functions: Vec<FunctionAbi>,
    pub registered_by: Address,
}

/// Describes one callable function so the explorer can decode calls.
#[contracttype]
#[derive(Clone)]
pub struct FunctionAbi {
    pub name: Symbol,        // e.g. symbol_short!("swap")
    pub description: String, // "Swap token_in for token_out"
    pub params: Vec<ParamDef>,
}

/// One parameter definition.
#[contracttype]
#[derive(Clone)]
pub struct ParamDef {
    pub name: Symbol,
    pub kind: Symbol, // "address" | "i128" | "symbol" | "bytes"
}

/// A decoded, human-readable event stored on-chain.
#[contracttype]
#[derive(Clone)]
pub struct DecodedEvent {
    pub seq: u64,
    pub contract_id: BytesN<32>,
    pub function: Symbol,
    pub ledger: u32,
    pub description: String, // "Address GA… swapped 100 USDC → 98.7 XLM"
    pub raw_topics: Vec<String>,
    pub raw_data: Bytes,
}

/// Event submission parameters (reduces function parameter count)
#[contracttype]
#[derive(Clone)]
pub struct EventInput {
    pub contract_id: BytesN<32>,
    pub function: Symbol,
    pub ledger: u32,
    pub description: String,
    pub raw_topics: Vec<String>,
    pub raw_data: Bytes,
}

// ── Contract ──────────────────────────────────────────────────────────────────
#[contract]
pub struct ExplorerContract;

#[contractimpl]
impl ExplorerContract {
    // ── Admin ─────────────────────────────────────────────────────────────────

    /// Initialise with an admin address (call once).
    pub fn init(env: Env, admin: Address) {
        if env.storage().instance().has(&DataKey::Admin) {
            panic_with_error!(&env, Error::AlreadyExists);
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::EventSeq, &0u64);
    }

    /// Transfer the admin role to a new address (current admin only).
    pub fn transfer_admin(env: Env, caller: Address, new_admin: Address) {
        caller.require_auth();
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        if caller != admin {
            panic_with_error!(&env, Error::Unauthorized);
        }
        env.storage().instance().set(&DataKey::Admin, &new_admin);
        env.events()
            .publish((symbol_short!("adm_xfer"), caller), new_admin);
    }

    // ── Contract Registry ─────────────────────────────────────────────────────

    /// Register ABI-like metadata for a Soroban contract.
    pub fn register_contract(
        env: Env,
        caller: Address,
        contract_id: BytesN<32>,
        meta: ContractMeta,
    ) {
        caller.require_auth();
        let key = DataKey::Contract(contract_id.clone());
        if env.storage().persistent().has(&key) {
            panic_with_error!(&env, Error::AlreadyExists);
        }
        env.storage().persistent().set(&key, &meta);
        env.events()
            .publish((symbol_short!("register"), contract_id), meta.name);
    }

    /// Update metadata (admin or original registrant only).
    pub fn update_contract(env: Env, caller: Address, contract_id: BytesN<32>, meta: ContractMeta) {
        caller.require_auth();
        let key = DataKey::Contract(contract_id.clone());
        let existing: ContractMeta = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| panic_with_error!(&env, Error::NotFound));

        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        if caller != existing.registered_by && caller != admin {
            panic_with_error!(&env, Error::Unauthorized);
        }
        env.storage().persistent().set(&key, &meta);
    }

    /// Fetch metadata for a contract.
    pub fn get_contract(env: Env, contract_id: BytesN<32>) -> ContractMeta {
        env.storage()
            .persistent()
            .get(&DataKey::Contract(contract_id))
            .unwrap_or_else(|| panic_with_error!(&env, Error::NotFound))
    }

    // ── Event Decoder ─────────────────────────────────────────────────────────

    /// Submit a decoded event (called by the off-chain indexer via a trusted tx).
    /// The indexer decodes raw XDR and calls this to persist a human-readable record.
    pub fn submit_event(env: Env, caller: Address, input: EventInput) {
        caller.require_auth();
        // Only admin or registered indexers may submit events.
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        if caller != admin {
            panic_with_error!(&env, Error::Unauthorized);
        }

        let seq: u64 = env
            .storage()
            .instance()
            .get(&DataKey::EventSeq)
            .unwrap_or(0);
        let event = DecodedEvent {
            seq,
            contract_id: input.contract_id.clone(),
            function: input.function.clone(),
            ledger: input.ledger,
            description: input.description.clone(),
            raw_topics: input.raw_topics,
            raw_data: input.raw_data,
        };
        env.storage()
            .persistent()
            .set(&DataKey::EventLog(seq), &event);
        env.storage().instance().set(&DataKey::EventSeq, &(seq + 1));

        env.events().publish(
            (symbol_short!("decoded"), input.contract_id, input.function),
            input.description,
        );
    }

    /// Fetch a single decoded event by sequence number.
    pub fn get_event(env: Env, seq: u64) -> DecodedEvent {
        env.storage()
            .persistent()
            .get(&DataKey::EventLog(seq))
            .unwrap_or_else(|| panic_with_error!(&env, Error::NotFound))
    }

    /// Return the total number of stored events.
    pub fn event_count(env: Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::EventSeq)
            .unwrap_or(0)
    }

    /// Fetch events starting from cursor (inclusive seq). Returns up to `limit` events.
    /// Use the last event's `seq + 1` as the next cursor.
    pub fn get_events(env: Env, cursor: u64, limit: u32) -> Vec<DecodedEvent> {
        let total: u64 = env
            .storage()
            .instance()
            .get(&DataKey::EventSeq)
            .unwrap_or(0);
        let mut out: Vec<DecodedEvent> = Vec::new(&env);
        let mut seq = cursor;
        while (out.len() as u32) < limit && seq < total {
            if let Some(ev) = env.storage().persistent().get(&DataKey::EventLog(seq)) {
                out.push_back(ev);
            }
            seq += 1;
        }
        out
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{testutils::Address as _, Env};

    fn setup() -> (Env, ExplorerContractClient<'static>) {
        let env = Env::default();
        env.mock_all_auths();
        let id = env.register_contract(None, ExplorerContract);
        let client = ExplorerContractClient::new(&env, &id);
        (env, client)
    }

    #[test]
    fn test_init_and_register() {
        let (env, client) = setup();
        let admin = Address::generate(&env);
        client.init(&admin);

        let cid: BytesN<32> = BytesN::from_array(&env, &[1u8; 32]);
        let meta = ContractMeta {
            version: 1,
            name: String::from_str(&env, "StellarSwap"),
            description: String::from_str(&env, "DEX on Stellar"),
            functions: Vec::new(&env),
            registered_by: admin.clone(),
        };
        client.register_contract(&admin, &cid, &meta);
        let fetched = client.get_contract(&cid);
        assert_eq!(fetched.name, String::from_str(&env, "StellarSwap"));
    }

    #[test]
    fn test_submit_and_get_event() {
        let (env, client) = setup();
        let admin = Address::generate(&env);
        client.init(&admin);

        let cid: BytesN<32> = BytesN::from_array(&env, &[2u8; 32]);
        let input = EventInput {
            contract_id: cid.clone(),
            function: symbol_short!("swap"),
            ledger: 4521983u32,
            description: String::from_str(
                &env,
                "Address GABC... swapped 100 USDC → 98.7 XLM on StellarSwap",
            ),
            raw_topics: Vec::new(&env),
            raw_data: Bytes::new(&env),
        };
        client.submit_event(&admin, &input);

        assert_eq!(client.event_count(), 1u64);
        let ev = client.get_event(&0u64);
        assert_eq!(ev.ledger, 4521983u32);
    }

    #[test]
    fn test_cursor_pagination() {
        let (env, client) = setup();
        let admin = Address::generate(&env);
        client.init(&admin);

        let cid: BytesN<32> = BytesN::from_array(&env, &[3u8; 32]);
        let base = EventInput {
            contract_id: cid.clone(),
            function: symbol_short!("swap"),
            ledger: 100u32,
            description: String::from_str(&env, "test"),
            raw_topics: Vec::new(&env),
            raw_data: Bytes::new(&env),
        };

        for i in 0..5 {
            client.submit_event(&admin, &base);
        }
        assert_eq!(client.event_count(), 5u64);

        let page1 = client.get_events(&0u64, &2u32);
        assert_eq!(page1.len(), 2);
        assert_eq!(page1.get(0).unwrap().seq, 0);
        assert_eq!(page1.get(1).unwrap().seq, 1);

        let page2 = client.get_events(&2u64, &2u32);
        assert_eq!(page2.len(), 2);
        assert_eq!(page2.get(0).unwrap().seq, 2);
        assert_eq!(page2.get(1).unwrap().seq, 3);

        let page3 = client.get_events(&4u64, &2u32);
        assert_eq!(page3.len(), 1);
        assert_eq!(page3.get(0).unwrap().seq, 4);

        let empty = client.get_events(&10u64, &5u32);
        assert_eq!(empty.len(), 0);
    }

    #[test]
    #[should_panic]
    fn test_double_init_panics() {
        let (env, client) = setup();
        let admin = Address::generate(&env);
        client.init(&admin);
        client.init(&admin); // should panic
    }

    #[test]
    fn test_transfer_admin_success() {
        let (env, client) = setup();
        let admin = Address::generate(&env);
        let new_admin = Address::generate(&env);
        client.init(&admin);

        client.transfer_admin(&admin, &new_admin);

        // New admin can submit events; if they weren't admin this would panic.
        let cid: BytesN<32> = BytesN::from_array(&env, &[9u8; 32]);
        let input = EventInput {
            contract_id: cid.clone(),
            function: symbol_short!("ping"),
            ledger: 1u32,
            description: String::from_str(&env, "new admin test"),
            raw_topics: Vec::new(&env),
            raw_data: Bytes::new(&env),
        };
        client.submit_event(&new_admin, &input);
        assert_eq!(client.event_count(), 1u64);
    }

    #[test]
    #[should_panic]
    fn test_transfer_admin_unauthorized() {
        let (env, client) = setup();
        let admin = Address::generate(&env);
        let attacker = Address::generate(&env);
        let new_admin = Address::generate(&env);
        client.init(&admin);

        // Non-admin caller must panic with Unauthorized.
        client.transfer_admin(&attacker, &new_admin);
    }

    #[test]
    #[should_panic]
    fn test_old_admin_loses_access_after_transfer() {
        let (env, client) = setup();
        let admin = Address::generate(&env);
        let new_admin = Address::generate(&env);
        client.init(&admin);
        client.transfer_admin(&admin, &new_admin);

        // Old admin is no longer privileged; submit_event must panic.
        let cid: BytesN<32> = BytesN::from_array(&env, &[10u8; 32]);
        let input = EventInput {
            contract_id: cid.clone(),
            function: symbol_short!("ping"),
            ledger: 1u32,
            description: String::from_str(&env, "stale admin attempt"),
            raw_topics: Vec::new(&env),
            raw_data: Bytes::new(&env),
        };
        client.submit_event(&admin, &input);
    }

    #[test]
    fn test_transfer_admin_to_self_is_noop() {
        let (env, client) = setup();
        let admin = Address::generate(&env);
        client.init(&admin);

        // Admin transferring to themselves keeps them as admin.
        client.transfer_admin(&admin, &admin);

        let cid: BytesN<32> = BytesN::from_array(&env, &[11u8; 32]);
        let input = EventInput {
            contract_id: cid.clone(),
            function: symbol_short!("ping"),
            ledger: 1u32,
            description: String::from_str(&env, "self transfer test"),
            raw_topics: Vec::new(&env),
            raw_data: Bytes::new(&env),
        };
        client.submit_event(&admin, &input);
        assert_eq!(client.event_count(), 1u64);
    }
}
