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
    pub fn submit_event(
        env: Env,
        caller: Address,
        contract_id: BytesN<32>,
        function: Symbol,
        ledger: u32,
        description: String,
        raw_topics: Vec<String>,
        raw_data: Bytes,
    ) {
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
            contract_id: contract_id.clone(),
            function: function.clone(),
            ledger,
            description: description.clone(),
            raw_topics,
            raw_data,
        };
        env.storage()
            .persistent()
            .set(&DataKey::EventLog(seq), &event);
        env.storage().instance().set(&DataKey::EventSeq, &(seq + 1));

        env.events().publish(
            (symbol_short!("decoded"), contract_id, function),
            description,
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

    /// Fetch a page of events [from, from+limit).
    pub fn get_events(env: Env, from: u64, limit: u32) -> Vec<DecodedEvent> {
        let total: u64 = env
            .storage()
            .instance()
            .get(&DataKey::EventSeq)
            .unwrap_or(0);
        let mut out: Vec<DecodedEvent> = Vec::new(&env);
        let end = (from + limit as u64).min(total);
        for seq in from..end {
            if let Some(ev) = env.storage().persistent().get(&DataKey::EventLog(seq)) {
                out.push_back(ev);
            }
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
        client.submit_event(
            &admin,
            &cid,
            &symbol_short!("swap"),
            &4521983u32,
            &String::from_str(
                &env,
                "Address GABC... swapped 100 USDC → 98.7 XLM on StellarSwap",
            ),
            &Vec::new(&env),
            &Bytes::new(&env),
        );

        assert_eq!(client.event_count(), 1u64);
        let ev = client.get_event(&0u64);
        assert_eq!(ev.ledger, 4521983u32);
    }

    #[test]
    #[should_panic]
    fn test_double_init_panics() {
        let (env, client) = setup();
        let admin = Address::generate(&env);
        client.init(&admin);
        client.init(&admin); // should panic
    }
}
