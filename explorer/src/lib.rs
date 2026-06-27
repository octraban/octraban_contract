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
    BelowFloor = 4,
}

// ── Storage keys ─────────────────────────────────────────────────────────────
#[contracttype]
#[derive(Clone)]
pub struct VersionKey {
    pub contract_id: BytesN<32>,
    pub abi_version: u32,
}

#[contracttype]
pub enum DataKey {
    Admin,
    Contract(BytesN<32>), // contract_id → ContractMeta (latest version)
    ContractVersion(VersionKey), // (contract_id, abi_version) → ContractMeta (version history)
    EventLog(u64),        // slot → DecodedEvent  (slot = seq % max_events)
    EventSeq,             // monotonic counter (never wraps)
    MaxEvents,            // u32 ring-buffer capacity
}

/// Minimum allowed value for max_events (prevents accidental data loss).
pub const MIN_MAX_EVENTS: u32 = 1_000;
/// Default ring-buffer capacity used at init when not overridden.
pub const DEFAULT_MAX_EVENTS: u32 = 50_000;

// ── Data types ────────────────────────────────────────────────────────────────

/// ABI-like metadata for a registered contract.
#[contracttype]
#[derive(Clone)]
pub struct ContractMeta {
    pub version: u32, // Metadata schema version for forward compatibility
    pub abi_version: u32, // ABI version, incremented on each update
    pub min_ledger: u32,  // First ledger at which this ABI version applies
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
    /// `max_events` is the ring-buffer capacity (default: 50_000).
    pub fn init(env: Env, admin: Address, max_events: u32) {
        if env.storage().instance().has(&DataKey::Admin) {
            panic_with_error!(&env, Error::AlreadyExists);
        }
        let cap = if max_events == 0 { DEFAULT_MAX_EVENTS } else { max_events };
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::EventSeq, &0u64);
        env.storage().instance().set(&DataKey::MaxEvents, &cap);
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
        // Set initial ABI version and min ledger, then persist
        let mut stored = meta;
        stored.abi_version = 0;
        stored.min_ledger = env.ledger().sequence();
        env.storage().persistent().set(&key, &stored);

        // Store version 0 in version history
        let vkey = DataKey::ContractVersion(VersionKey {
            contract_id: contract_id.clone(),
            abi_version: 0,
        });
        env.storage().persistent().set(&vkey, &stored);

        // #275 — emit contract_registered diagnostic event
        env.events().publish(
            (symbol_short!("c_reg"), contract_id.clone()),
            (stored.registered_by.clone(), stored.version, env.ledger().sequence()),
        );
        // legacy event kept for off-chain indexers already subscribed to "register"
        env.events()
            .publish((symbol_short!("register"), contract_id), stored.name);
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

        // #272 — optimistic concurrency guard: submitted abi_version must be current + 1
        let expected = existing.abi_version + 1;
        if meta.abi_version != expected {
            panic_with_error!(&env, Error::Unauthorized);
        }

        let old_abi_version = existing.abi_version;
        let old_version = existing.version;
        let new_abi_version = meta.abi_version;
        let min_ledger = env.ledger().sequence();
        let mut updated = meta;
        updated.min_ledger = min_ledger;
        env.storage().persistent().set(&key, &updated);

        // #272 — store version in history
        let vkey = DataKey::ContractVersion(VersionKey {
            contract_id: contract_id.clone(),
            abi_version: new_abi_version,
        });
        env.storage().persistent().set(&vkey, &updated);

        // #272 — emit ContractAbiUpdated diagnostic event
        env.events().publish(
            (symbol_short!("c_abiu"), contract_id.clone()),
            (old_abi_version, new_abi_version, min_ledger),
        );

        // #275 — emit contract_updated diagnostic event
        env.events().publish(
            (symbol_short!("c_upd"), contract_id),
            (caller, old_version, updated.version, env.ledger().sequence()),
        );
    }

    /// Fetch metadata for a contract (returns None if not found).
    pub fn get_contract(env: Env, contract_id: BytesN<32>) -> Option<ContractMeta> {
        env.storage()
            .persistent()
            .get(&DataKey::Contract(contract_id))
    }

    /// Fetch a specific ABI version's metadata (returns None if not found).
    pub fn get_contract_version(env: Env, contract_id: BytesN<32>, abi_version: u32) -> Option<ContractMeta> {
        env.storage()
            .persistent()
            .get(&DataKey::ContractVersion(VersionKey { contract_id, abi_version }))
    }

    /// Fetch the latest metadata for a contract (alias for get_contract).
    pub fn get_latest_contract(env: Env, contract_id: BytesN<32>) -> Option<ContractMeta> {
        env.storage()
            .persistent()
            .get(&DataKey::Contract(contract_id))
    }

    /// Deregister a contract (registrant or admin only). Removes the entry from
    /// persistent storage and emits a ContractDeregistered event.
    pub fn deregister_contract(env: Env, caller: Address, contract_id: BytesN<32>) {
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

        // Remove the latest entry
        env.storage().persistent().remove(&key);

        // #271 — emit ContractDeregistered diagnostic event
        env.events().publish(
            (symbol_short!("c_dereg"), contract_id),
            (caller, env.ledger().sequence()),
        );
    }

    // ── Event cap management ──────────────────────────────────────────────────

    /// Admin-only: change the ring-buffer cap.  Floor: MIN_MAX_EVENTS.
    pub fn set_max_events(env: Env, caller: Address, new_max: u32) {
        caller.require_auth();
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        if caller != admin {
            panic_with_error!(&env, Error::Unauthorized);
        }
        if new_max < MIN_MAX_EVENTS {
            panic_with_error!(&env, Error::BelowFloor);
        }
        env.storage().instance().set(&DataKey::MaxEvents, &new_max);
    }

    /// Return `(current_count, max_events)` for monitoring.
    pub fn storage_utilisation(env: Env) -> (u64, u32) {
        let seq: u64 = env.storage().instance().get(&DataKey::EventSeq).unwrap_or(0);
        let max: u32 = env
            .storage()
            .instance()
            .get(&DataKey::MaxEvents)
            .unwrap_or(DEFAULT_MAX_EVENTS);
        let count = seq.min(max as u64);
        (count, max)
    }

    // ── Event Decoder ─────────────────────────────────────────────────────────

    /// Submit a decoded event (called by the off-chain indexer via a trusted tx).
    /// Uses a ring-buffer: when seq >= max_events, overwrites the oldest slot.
    pub fn submit_event(env: Env, caller: Address, input: EventInput) {
        caller.require_auth();
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        if caller != admin {
            panic_with_error!(&env, Error::Unauthorized);
        }

        let seq: u64 = env
            .storage()
            .instance()
            .get(&DataKey::EventSeq)
            .unwrap_or(0);
        let max: u32 = env
            .storage()
            .instance()
            .get(&DataKey::MaxEvents)
            .unwrap_or(DEFAULT_MAX_EVENTS);

        // Ring-buffer slot: wraps when seq >= max_events
        let slot = seq % (max as u64);
        let evicting = seq >= (max as u64);
        let evicted_seq = if evicting { seq - (max as u64) } else { seq };

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
            .set(&DataKey::EventLog(slot), &event);
        env.storage().instance().set(&DataKey::EventSeq, &(seq + 1));

        // #275 — emit event_submitted diagnostic event
        env.events().publish(
            (symbol_short!("ev_sub"), input.contract_id.clone(), input.function.clone()),
            (seq, input.ledger),
        );

        // #274 — emit StorageCapReached when eviction occurs
        if evicting {
            env.events().publish(
                (symbol_short!("cap_hit"),),
                (evicted_seq, seq),
            );
        }

        // legacy event kept for backward compat
        env.events().publish(
            (symbol_short!("decoded"), input.contract_id, input.function),
            input.description,
        );
    }

    /// Fetch a single decoded event by sequence number.
    /// `seq` must be within the live ring-buffer window.
    pub fn get_event(env: Env, seq: u64) -> DecodedEvent {
        let max: u32 = env
            .storage()
            .instance()
            .get(&DataKey::MaxEvents)
            .unwrap_or(DEFAULT_MAX_EVENTS);
        let slot = seq % (max as u64);
        let stored: DecodedEvent = env
            .storage()
            .persistent()
            .get(&DataKey::EventLog(slot))
            .unwrap_or_else(|| panic_with_error!(&env, Error::NotFound));
        // Verify the slot still holds the requested seq (not overwritten)
        if stored.seq != seq {
            panic_with_error!(&env, Error::NotFound);
        }
        stored
    }

    /// Return the total number of stored events (capped at max_events).
    pub fn event_count(env: Env) -> u64 {
        let seq: u64 = env
            .storage()
            .instance()
            .get(&DataKey::EventSeq)
            .unwrap_or(0);
        let max: u32 = env
            .storage()
            .instance()
            .get(&DataKey::MaxEvents)
            .unwrap_or(DEFAULT_MAX_EVENTS);
        seq.min(max as u64)
    }

    /// Fetch events starting from cursor (inclusive seq). Returns up to `limit` events.
    /// Skips slots that have been overwritten by the ring-buffer.
    pub fn get_events(env: Env, cursor: u64, limit: u32) -> Vec<DecodedEvent> {
        let total_seq: u64 = env
            .storage()
            .instance()
            .get(&DataKey::EventSeq)
            .unwrap_or(0);
        let max: u32 = env
            .storage()
            .instance()
            .get(&DataKey::MaxEvents)
            .unwrap_or(DEFAULT_MAX_EVENTS);
        // Oldest available seq in the ring (once the buffer has wrapped)
        let oldest = if total_seq > (max as u64) {
            total_seq - (max as u64)
        } else {
            0
        };
        let start = cursor.max(oldest);
        let mut out: Vec<DecodedEvent> = Vec::new(&env);
        let mut seq = start;
        while (out.len() as u32) < limit && seq < total_seq {
            let slot = seq % (max as u64);
            if let Some(ev) = env.storage().persistent().get::<DataKey, DecodedEvent>(&DataKey::EventLog(slot)) {
                if ev.seq == seq {
                    out.push_back(ev);
                }
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
    use soroban_sdk::{testutils::{Address as _, Events as _}, Env};

    fn setup() -> (Env, ExplorerContractClient<'static>) {
        let env = Env::default();
        env.mock_all_auths();
        let id = env.register_contract(None, ExplorerContract);
        let client = ExplorerContractClient::new(&env, &id);
        (env, client)
    }

    fn make_input(env: &Env, cid: &BytesN<32>) -> EventInput {
        EventInput {
            contract_id: cid.clone(),
            function: symbol_short!("swap"),
            ledger: 100u32,
            description: String::from_str(env, "test"),
            raw_topics: Vec::new(env),
            raw_data: Bytes::new(env),
        }
    }

    #[test]
    fn test_init_and_register() {
        let (env, client) = setup();
        let admin = Address::generate(&env);
        client.init(&admin, &0u32);

        let cid: BytesN<32> = BytesN::from_array(&env, &[1u8; 32]);
        let meta = ContractMeta {
            version: 1,
            abi_version: 0,
            min_ledger: 0,
            name: String::from_str(&env, "StellarSwap"),
            description: String::from_str(&env, "DEX on Stellar"),
            functions: Vec::new(&env),
            registered_by: admin.clone(),
        };
        client.register_contract(&admin, &cid, &meta);
        let fetched = client.get_contract(&cid).unwrap();
        assert_eq!(fetched.name, String::from_str(&env, "StellarSwap"));
    }

    #[test]
    fn test_submit_and_get_event() {
        let (env, client) = setup();
        let admin = Address::generate(&env);
        client.init(&admin, &0u32);

        let cid: BytesN<32> = BytesN::from_array(&env, &[2u8; 32]);
        let input = EventInput {
            contract_id: cid.clone(),
            function: symbol_short!("swap"),
            ledger: 4521983u32,
            description: String::from_str(
                &env,
                "Address GABC... swapped 100 USDC",
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
        client.init(&admin, &0u32);

        let cid: BytesN<32> = BytesN::from_array(&env, &[3u8; 32]);
        let base = make_input(&env, &cid);

        for _ in 0..5 {
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
        client.init(&admin, &0u32);
        client.init(&admin, &0u32); // should panic
    }

    // ── Ring-buffer cap tests (#274) ──────────────────────────────────────────

    #[test]
    fn test_ring_buffer_wraps_correctly() {
        let (env, client) = setup();
        let admin = Address::generate(&env);
        // tiny cap of 5 for fast testing
        client.init(&admin, &5u32);
        let cid: BytesN<32> = BytesN::from_array(&env, &[10u8; 32]);
        let base = make_input(&env, &cid);

        // fill to cap
        for _ in 0..5 {
            client.submit_event(&admin, &base);
        }
        assert_eq!(client.event_count(), 5u64);

        // overfill by 10
        for _ in 0..10 {
            client.submit_event(&admin, &base);
        }
        // count must not exceed cap
        assert_eq!(client.event_count(), 5u64);

        // oldest available seq is 10 (15 total - cap 5)
        let evs = client.get_events(&0u64, &20u32);
        assert_eq!(evs.len(), 5);
        assert_eq!(evs.get(0).unwrap().seq, 10);
        assert_eq!(evs.get(4).unwrap().seq, 14);
    }

    #[test]
    fn test_storage_utilisation() {
        let (env, client) = setup();
        let admin = Address::generate(&env);
        client.init(&admin, &1000u32);
        let (cur, max) = client.storage_utilisation();
        assert_eq!(cur, 0u64);
        assert_eq!(max, 1000u32);
    }

    #[test]
    #[should_panic]
    fn test_set_max_events_below_floor_rejected() {
        let (env, client) = setup();
        let admin = Address::generate(&env);
        client.init(&admin, &0u32);
        client.set_max_events(&admin, &999u32); // 999 < MIN_MAX_EVENTS=1000 → panic
    }

    #[test]
    fn test_set_max_events_accepted() {
        let (env, client) = setup();
        let admin = Address::generate(&env);
        client.init(&admin, &0u32);
        client.set_max_events(&admin, &1000u32);
        let (_, max) = client.storage_utilisation();
        assert_eq!(max, 1000u32);
    }

    // ── Diagnostic event emission tests (#275) ────────────────────────────────

    #[test]
    fn test_register_emits_event() {
        let (env, client) = setup();
        let admin = Address::generate(&env);
        client.init(&admin, &0u32);

        let cid: BytesN<32> = BytesN::from_array(&env, &[20u8; 32]);
        let meta = ContractMeta {
            version: 1,
            abi_version: 0,
            min_ledger: 0,
            name: String::from_str(&env, "TestDex"),
            description: String::from_str(&env, "desc"),
            functions: Vec::new(&env),
            registered_by: admin.clone(),
        };
        client.register_contract(&admin, &cid, &meta);

        // At least 2 events emitted: c_reg + register
        let evs = env.events().all();
        assert!(evs.len() >= 2);
    }

    #[test]
    fn test_update_emits_event() {
        let (env, client) = setup();
        let admin = Address::generate(&env);
        client.init(&admin, &0u32);

        let cid: BytesN<32> = BytesN::from_array(&env, &[21u8; 32]);
        let meta_v1 = ContractMeta {
            version: 1,
            abi_version: 0,
            min_ledger: 0,
            name: String::from_str(&env, "Dex"),
            description: String::from_str(&env, "v1"),
            functions: Vec::new(&env),
            registered_by: admin.clone(),
        };
        client.register_contract(&admin, &cid, &meta_v1);
        let before = env.events().all().len();

        let meta_v2 = ContractMeta { version: 2, abi_version: 1, ..meta_v1 };
        client.update_contract(&admin, &cid, &meta_v2);

        // c_upd event emitted
        assert!(env.events().all().len() > before);
    }

    #[test]
    fn test_submit_emits_ev_sub_event() {
        let (env, client) = setup();
        let admin = Address::generate(&env);
        client.init(&admin, &0u32);

        let cid: BytesN<32> = BytesN::from_array(&env, &[22u8; 32]);
        client.submit_event(&admin, &make_input(&env, &cid));

        let evs = env.events().all();
        // ev_sub + decoded events emitted
        assert!(evs.len() >= 2);
    }

    #[test]
    fn test_cap_hit_event_emitted_on_eviction() {
        let (env, client) = setup();
        let admin = Address::generate(&env);
        client.init(&admin, &5u32);
        let cid: BytesN<32> = BytesN::from_array(&env, &[23u8; 32]);
        let base = make_input(&env, &cid);

        for _ in 0..5 {
            client.submit_event(&admin, &base);
        }
        let before = env.events().all().len();
        // This submit triggers eviction
        client.submit_event(&admin, &base);
        assert!(env.events().all().len() > before);
    }

    // ── ABI versioning tests (#272) ────────────────────────────────────────────

    #[test]
    fn test_register_sets_version_zero() {
        let (env, client) = setup();
        let admin = Address::generate(&env);
        client.init(&admin, &0u32);

        let cid: BytesN<32> = BytesN::from_array(&env, &[30u8; 32]);
        let meta = ContractMeta {
            version: 1,
            abi_version: 99, // should be overwritten to 0
            min_ledger: 0,
            name: String::from_str(&env, "Test"),
            description: String::from_str(&env, "desc"),
            functions: Vec::new(&env),
            registered_by: admin.clone(),
        };
        client.register_contract(&admin, &cid, &meta);

        let fetched = client.get_contract(&cid).unwrap();
        assert_eq!(fetched.abi_version, 0);

        // Version 0 should also be retrievable via get_contract_version
        let v0 = client.get_contract_version(&cid, &0u32).unwrap();
        assert_eq!(v0.abi_version, 0);
        assert_eq!(v0.name, String::from_str(&env, "Test"));
    }

    #[test]
    fn test_sequential_updates_increment_abi_version() {
        let (env, client) = setup();
        let admin = Address::generate(&env);
        client.init(&admin, &0u32);

        let cid: BytesN<32> = BytesN::from_array(&env, &[31u8; 32]);
        let meta_v0 = ContractMeta {
            version: 1,
            abi_version: 0,
            min_ledger: 0,
            name: String::from_str(&env, "App"),
            description: String::from_str(&env, "v0"),
            functions: Vec::new(&env),
            registered_by: admin.clone(),
        };
        client.register_contract(&admin, &cid, &meta_v0);

        // Update to abi_version 1
        let meta_v1 = ContractMeta {
            version: 1,
            abi_version: 1,
            ..meta_v0.clone()
        };
        client.update_contract(&admin, &cid, &meta_v1);
        let latest = client.get_contract(&cid).unwrap();
        assert_eq!(latest.abi_version, 1);

        // Update to abi_version 2
        let meta_v2 = ContractMeta {
            version: 1,
            abi_version: 2,
            ..meta_v0
        };
        client.update_contract(&admin, &cid, &meta_v2);
        let latest = client.get_contract(&cid).unwrap();
        assert_eq!(latest.abi_version, 2);

        // All versions retrievable via get_contract_version
        assert!(client.get_contract_version(&cid, &0u32).is_some());
        assert!(client.get_contract_version(&cid, &1u32).is_some());
        assert!(client.get_contract_version(&cid, &2u32).is_some());
        assert!(client.get_contract_version(&cid, &3u32).is_none());
    }

    #[test]
    #[should_panic]
    fn test_stale_write_rejected() {
        let (env, client) = setup();
        let admin = Address::generate(&env);
        client.init(&admin, &0u32);

        let cid: BytesN<32> = BytesN::from_array(&env, &[32u8; 32]);
        let meta_v0 = ContractMeta {
            version: 1,
            abi_version: 0,
            min_ledger: 0,
            name: String::from_str(&env, "X"),
            description: String::from_str(&env, "x"),
            functions: Vec::new(&env),
            registered_by: admin.clone(),
        };
        client.register_contract(&admin, &cid, &meta_v0);

        // First update to v1
        let meta_v1 = ContractMeta {
            version: 1,
            abi_version: 1,
            ..meta_v0.clone()
        };
        client.update_contract(&admin, &cid, &meta_v1);

        // Stale write: try to write v1 again (should be v2)
        let meta_stale = ContractMeta {
            version: 1,
            abi_version: 1,
            ..meta_v0
        };
        client.update_contract(&admin, &cid, &meta_stale);
    }

    #[test]
    fn test_get_contract_returns_none_for_missing() {
        let (env, client) = setup();
        let admin = Address::generate(&env);
        client.init(&admin, &0u32);

        let cid: BytesN<32> = BytesN::from_array(&env, &[33u8; 32]);
        assert!(client.get_contract(&cid).is_none());
        assert!(client.get_latest_contract(&cid).is_none());
        assert!(client.get_contract_version(&cid, &0u32).is_none());
    }

    // ── Deregistration tests (#271) ────────────────────────────────────────────

    #[test]
    fn test_admin_deregisters_contract() {
        let (env, client) = setup();
        let admin = Address::generate(&env);
        client.init(&admin, &0u32);

        let cid: BytesN<32> = BytesN::from_array(&env, &[40u8; 32]);
        let meta = ContractMeta {
            version: 1,
            abi_version: 0,
            min_ledger: 0,
            name: String::from_str(&env, "ToRemove"),
            description: String::from_str(&env, "will be removed"),
            functions: Vec::new(&env),
            registered_by: admin.clone(),
        };
        client.register_contract(&admin, &cid, &meta);
        assert!(client.get_contract(&cid).is_some());

        // Admin deregisters
        client.deregister_contract(&admin, &cid);
        assert!(client.get_contract(&cid).is_none());
    }

    #[test]
    fn test_registrant_deregisters_contract() {
        let (env, client) = setup();
        let admin = Address::generate(&env);
        client.init(&admin, &0u32);

        let registrant = Address::generate(&env);
        let cid: BytesN<32> = BytesN::from_array(&env, &[41u8; 32]);
        let meta = ContractMeta {
            version: 1,
            abi_version: 0,
            min_ledger: 0,
            name: String::from_str(&env, "RegOwned"),
            description: String::from_str(&env, "owned"),
            functions: Vec::new(&env),
            registered_by: registrant.clone(),
        };
        env.mock_all_auths();
        client.register_contract(&registrant, &cid, &meta);
        client.deregister_contract(&registrant, &cid);
        assert!(client.get_contract(&cid).is_none());
    }

    #[test]
    #[should_panic]
    fn test_stranger_cannot_deregister() {
        let (env, client) = setup();
        let admin = Address::generate(&env);
        client.init(&admin, &0u32);

        let registrant = Address::generate(&env);
        let stranger = Address::generate(&env);
        let cid: BytesN<32> = BytesN::from_array(&env, &[42u8; 32]);
        let meta = ContractMeta {
            version: 1,
            abi_version: 0,
            min_ledger: 0,
            name: String::from_str(&env, "Secure"),
            description: String::from_str(&env, "protected"),
            functions: Vec::new(&env),
            registered_by: registrant.clone(),
        };
        env.mock_all_auths();
        client.register_contract(&registrant, &cid, &meta);
        // Stranger tries to deregister — should panic
        client.deregister_contract(&stranger, &cid);
    }

    #[test]
    #[should_panic]
    fn test_deregister_missing_id_panics() {
        let (env, client) = setup();
        let admin = Address::generate(&env);
        client.init(&admin, &0u32);

        let cid: BytesN<32> = BytesN::from_array(&env, &[99u8; 32]);
        client.deregister_contract(&admin, &cid);
    }

    #[test]
    fn test_deregister_emits_event() {
        let (env, client) = setup();
        let admin = Address::generate(&env);
        client.init(&admin, &0u32);

        let cid: BytesN<32> = BytesN::from_array(&env, &[43u8; 32]);
        let meta = ContractMeta {
            version: 1,
            abi_version: 0,
            min_ledger: 0,
            name: String::from_str(&env, "EventTest"),
            description: String::from_str(&env, "emits event"),
            functions: Vec::new(&env),
            registered_by: admin.clone(),
        };
        client.register_contract(&admin, &cid, &meta);
        let before = env.events().all().len();
        client.deregister_contract(&admin, &cid);
        assert!(env.events().all().len() > before);
    }
}
