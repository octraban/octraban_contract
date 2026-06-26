#![no_std]

//! Explorer contract for registering contract metadata and persisting decoded
//! Soroban events in a compact on-chain ring buffer.

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, panic_with_error, symbol_short, Address,
    Bytes, BytesN, Env, String, Symbol, Vec,
};

// ── Error codes ──────────────────────────────────────────────────────────────
/// Contract errors emitted by the explorer contract.
#[allow(missing_docs)]
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    /// The requested contract or event could not be found.
    NotFound = 1,
    /// The caller is not authorized to perform the requested action.
    Unauthorized = 2,
    /// The requested registry entry already exists.
    AlreadyExists = 3,
    /// The requested event buffer size is below the minimum allowed value.
    BelowFloor = 4,
}

// ── Storage keys ─────────────────────────────────────────────────────────────
/// Storage keys used by the explorer contract.
#[allow(missing_docs)]
#[contracttype]
pub enum DataKey {
    /// The admin address that can submit decoded events.
    Admin,
    /// Maps a contract ID to its registered metadata.
    Contract(BytesN<32>), // contract_id → ContractMeta
    /// Stores the decoded event for a given event-log slot.
    EventLog(u64), // slot → DecodedEvent  (slot = seq % max_events)
    /// Tracks the monotonic event sequence counter.
    EventSeq, // monotonic counter (never wraps)
    /// Tracks the configured ring-buffer capacity.
    MaxEvents, // u32 ring-buffer capacity
}

/// Minimum allowed value for max_events (prevents accidental data loss).
pub const MIN_MAX_EVENTS: u32 = 1_000;
/// Default ring-buffer capacity used at init when not overridden.
pub const DEFAULT_MAX_EVENTS: u32 = 50_000;

// ── Data types ────────────────────────────────────────────────────────────────

/// ABI-like metadata for a registered contract.
#[allow(missing_docs)]
#[contracttype]
#[derive(Clone)]
pub struct ContractMeta {
    /// Metadata schema version for forward compatibility.
    pub version: u32,
    /// Human-readable contract name, for example `StellarSwap`.
    pub name: String,
    /// Human-readable description of the contract.
    pub description: String,
    /// Callable functions exposed by the registered contract.
    pub functions: Vec<FunctionAbi>,
    /// The address that originally registered the metadata.
    pub registered_by: Address,
}

/// Describes one callable function so the explorer can decode calls.
#[allow(missing_docs)]
#[contracttype]
#[derive(Clone)]
pub struct FunctionAbi {
    /// The function name as a Soroban symbol.
    pub name: Symbol,
    /// A human-readable description of the function.
    pub description: String,
    /// Parameter definitions for the function.
    pub params: Vec<ParamDef>,
}

/// One parameter definition.
#[allow(missing_docs)]
#[contracttype]
#[derive(Clone)]
pub struct ParamDef {
    /// The parameter name.
    pub name: Symbol,
    /// The parameter kind, such as `address`, `i128`, `symbol`, or `bytes`.
    pub kind: Symbol,
}

/// A decoded, human-readable event stored on-chain.
#[allow(missing_docs)]
#[contracttype]
#[derive(Clone)]
pub struct DecodedEvent {
    /// The monotonic sequence number assigned to the event.
    pub seq: u64,
    /// The contract ID that emitted the event.
    pub contract_id: BytesN<32>,
    /// The function name associated with the event.
    pub function: Symbol,
    /// The ledger sequence where the event was observed.
    pub ledger: u32,
    /// The decoded human-readable description.
    pub description: String,
    /// The raw event topics as strings.
    pub raw_topics: Vec<String>,
    /// The raw event data payload.
    pub raw_data: Bytes,
}

/// Event submission parameters used to reduce the number of function arguments.
#[allow(missing_docs)]
#[contracttype]
#[derive(Clone)]
pub struct EventInput {
    /// The contract ID that emitted the event.
    pub contract_id: BytesN<32>,
    /// The function name associated with the event.
    pub function: Symbol,
    /// The ledger sequence where the event was observed.
    pub ledger: u32,
    /// The decoded human-readable description.
    pub description: String,
    /// The raw event topics as strings.
    pub raw_topics: Vec<String>,
    /// The raw event data payload.
    pub raw_data: Bytes,
}

// ── Contract ──────────────────────────────────────────────────────────────────
/// The explorer contract used to register contract metadata and persist decoded events.
#[allow(missing_docs)]
#[contract]
pub struct ExplorerContract;

#[contractimpl]
impl ExplorerContract {
    // ── Admin ─────────────────────────────────────────────────────────────────

    /// Initializes the explorer contract and configures the initial event-ring buffer.
    ///
    /// # Arguments
    /// * `env` - The contract environment.
    /// * `admin` - The address that is authorized to submit decoded events and manage the ring buffer.
    /// * `max_events` - The initial maximum number of stored events. Use `0` to fall back to the default capacity.
    ///
    /// # Returns
    /// This function returns nothing.
    ///
    /// # Errors
    /// * `AlreadyExists` - If the contract has already been initialized.
    ///
    /// # Authorization
    /// No prior authorization is required. This function can be invoked once during bootstrap.
    ///
    /// # Examples
    /// ```bash
    /// stellar contract invoke --network testnet --id <CONTRACT_ID> --source alice -- init --admin GBK...
    /// ```
    pub fn init(env: Env, admin: Address, max_events: u32) {
        if env.storage().instance().has(&DataKey::Admin) {
            panic_with_error!(&env, Error::AlreadyExists);
        }
        let cap = if max_events == 0 {
            DEFAULT_MAX_EVENTS
        } else {
            max_events
        };
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::EventSeq, &0u64);
        env.storage().instance().set(&DataKey::MaxEvents, &cap);
    }

    // ── Contract Registry ─────────────────────────────────────────────────────

    /// Registers ABI-like metadata for a Soroban contract.
    ///
    /// # Arguments
    /// * `env` - The contract environment.
    /// * `caller` - The address that must authorize the registration.
    /// * `contract_id` - The contract identifier for the registered contract.
    /// * `meta` - The metadata payload that describes the contract ABI.
    ///
    /// # Returns
    /// This function returns nothing.
    ///
    /// # Errors
    /// This function does not return any contract error variants.
    ///
    /// # Authorization
    /// The `caller` address must authorize the invocation.
    ///
    /// # Examples
    /// ```bash
    /// stellar contract invoke --network testnet --id <CONTRACT_ID> --source alice -- register-contract --caller GBK... --contract-id <CONTRACT_ID> --meta '{"name":"Example"}'
    /// ```
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

        // #275 — emit contract_registered diagnostic event
        env.events().publish(
            (symbol_short!("c_reg"), contract_id.clone()),
            (
                meta.registered_by.clone(),
                meta.version,
                env.ledger().sequence(),
            ),
        );
        // legacy event kept for off-chain indexers already subscribed to "register"
        env.events()
            .publish((symbol_short!("register"), contract_id), meta.name);
    }

    /// Updates registered metadata when called by the admin or the original registrant.
    ///
    /// # Arguments
    /// * `env` - The contract environment.
    /// * `caller` - The address attempting to update the metadata.
    /// * `contract_id` - The target contract identifier.
    /// * `meta` - The replacement metadata payload.
    ///
    /// # Returns
    /// This function returns nothing.
    ///
    /// # Errors
    /// * `NotFound` - If the target contract is not registered.
    /// * `Unauthorized` - If the caller is neither the original registrant nor the admin.
    ///
    /// # Authorization
    /// The caller must be the admin or the original registrant that created the metadata entry.
    ///
    /// # Examples
    /// ```bash
    /// stellar contract invoke --network testnet --id <CONTRACT_ID> --source alice -- update-contract --caller GBK... --contract-id <CONTRACT_ID> --meta '{"name":"Updated"}'
    /// ```
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
        let old_version = existing.version;
        env.storage().persistent().set(&key, &meta);

        // #275 — emit contract_updated diagnostic event
        env.events().publish(
            (symbol_short!("c_upd"), contract_id),
            (caller, old_version, meta.version, env.ledger().sequence()),
        );
    }

    /// Fetches the registered metadata for a contract.
    ///
    /// # Arguments
    /// * `env` - The contract environment.
    /// * `contract_id` - The contract identifier to look up.
    ///
    /// # Returns
    /// The registered metadata for the contract on success.
    ///
    /// # Errors
    /// * `NotFound` - If no metadata has been registered for the contract.
    ///
    /// # Authorization
    /// This read-only function does not require authorization.
    ///
    /// # Examples
    /// ```bash
    /// stellar contract invoke --network testnet --id <CONTRACT_ID> --source alice -- get-contract --contract-id <CONTRACT_ID>
    /// ```
    pub fn get_contract(env: Env, contract_id: BytesN<32>) -> ContractMeta {
        env.storage()
            .persistent()
            .get(&DataKey::Contract(contract_id))
            .unwrap_or_else(|| panic_with_error!(&env, Error::NotFound))
    }

    // ── Event cap management ──────────────────────────────────────────────────

    /// Updates the ring-buffer capacity for persisted events.
    ///
    /// # Arguments
    /// * `env` - The contract environment.
    /// * `caller` - The address attempting to change the capacity.
    /// * `new_max` - The new maximum number of retained events.
    ///
    /// # Returns
    /// This function returns nothing.
    ///
    /// # Errors
    /// * `Unauthorized` - If the caller is not the configured admin.
    /// * `BelowFloor` - If the requested capacity is lower than `MIN_MAX_EVENTS`.
    ///
    /// # Authorization
    /// Only the configured admin may change the ring-buffer capacity.
    ///
    /// # Examples
    /// ```bash
    /// stellar contract invoke --network testnet --id <CONTRACT_ID> --source alice -- set-max-events --caller GBK... --new-max 100000
    /// ```
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

    /// Returns the current storage usage and configured event-buffer capacity.
    ///
    /// # Arguments
    /// * `env` - The contract environment.
    ///
    /// # Returns
    /// A tuple of `(current_count, max_events)` describing the current ring-buffer occupancy and configured capacity.
    ///
    /// # Errors
    /// This function does not return any contract error variants.
    ///
    /// # Authorization
    /// This read-only function does not require authorization.
    ///
    /// # Examples
    /// ```bash
    /// stellar contract invoke --network testnet --id <CONTRACT_ID> --source alice -- storage-utilisation
    /// ```
    pub fn storage_utilisation(env: Env) -> (u64, u32) {
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
        let count = seq.min(max as u64);
        (count, max)
    }

    // ── Event Decoder ─────────────────────────────────────────────────────────

    /// Submits a decoded event for on-chain storage in the explorer ring buffer.
    ///
    /// # Arguments
    /// * `env` - The contract environment.
    /// * `caller` - The address attempting to submit the event.
    /// * `input` - The decoded event payload to store.
    ///
    /// # Returns
    /// This function returns nothing.
    ///
    /// # Errors
    /// * `Unauthorized` - If the caller is not the configured admin.
    ///
    /// # Authorization
    /// Only the configured admin may submit decoded events.
    ///
    /// # Examples
    /// ```bash
    /// stellar contract invoke --network testnet --id <CONTRACT_ID> --source alice -- submit-event --caller GBK... --input '{"contract_id":"<CONTRACT_ID>","function":"transfer","ledger":123,"description":"Transferred tokens","raw_topics":[],"raw_data":""}'
    /// ```
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
            (
                symbol_short!("ev_sub"),
                input.contract_id.clone(),
                input.function.clone(),
            ),
            (seq, input.ledger),
        );

        // #274 — emit StorageCapReached when eviction occurs
        if evicting {
            env.events()
                .publish((symbol_short!("cap_hit"),), (evicted_seq, seq));
        }

        // legacy event kept for backward compat
        env.events().publish(
            (symbol_short!("decoded"), input.contract_id, input.function),
            input.description,
        );
    }

    /// Fetches a single decoded event by sequence number.
    ///
    /// # Arguments
    /// * `env` - The contract environment.
    /// * `seq` - The event sequence number to look up.
    ///
    /// # Returns
    /// The requested decoded event on success.
    ///
    /// # Errors
    /// * `NotFound` - If the requested sequence number is outside the live ring-buffer window or no event exists at that slot.
    ///
    /// # Authorization
    /// This read-only function does not require authorization.
    ///
    /// # Examples
    /// ```bash
    /// stellar contract invoke --network testnet --id <CONTRACT_ID> --source alice -- get-event --seq 42
    /// ```
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

    /// Returns the total number of stored events, capped by the configured maximum.
    ///
    /// # Arguments
    /// * `env` - The contract environment.
    ///
    /// # Returns
    /// The number of retained decoded events.
    ///
    /// # Errors
    /// This function does not return any contract error variants.
    ///
    /// # Authorization
    /// This read-only function does not require authorization.
    ///
    /// # Examples
    /// ```bash
    /// stellar contract invoke --network testnet --id <CONTRACT_ID> --source alice -- event-count
    /// ```
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

    /// Fetches decoded events starting from a cursor sequence number.
    ///
    /// # Arguments
    /// * `env` - The contract environment.
    /// * `cursor` - The first sequence number to consider.
    /// * `limit` - The maximum number of events to return.
    ///
    /// # Returns
    /// A vector of decoded events starting at or after the requested cursor, capped by `limit`.
    ///
    /// # Errors
    /// This function does not return any contract error variants.
    ///
    /// # Authorization
    /// This read-only function does not require authorization.
    ///
    /// # Examples
    /// ```bash
    /// stellar contract invoke --network testnet --id <CONTRACT_ID> --source alice -- get-events --cursor 0 --limit 10
    /// ```
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
            if let Some(ev) = env
                .storage()
                .persistent()
                .get::<DataKey, DecodedEvent>(&DataKey::EventLog(slot))
            {
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
    use soroban_sdk::{
        testutils::{Address as _, Events as _},
        Env,
    };

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
        client.init(&admin, &0u32);

        let cid: BytesN<32> = BytesN::from_array(&env, &[2u8; 32]);
        let input = EventInput {
            contract_id: cid.clone(),
            function: symbol_short!("swap"),
            ledger: 4521983u32,
            description: String::from_str(&env, "Address GABC... swapped 100 USDC"),
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
            name: String::from_str(&env, "Dex"),
            description: String::from_str(&env, "v1"),
            functions: Vec::new(&env),
            registered_by: admin.clone(),
        };
        client.register_contract(&admin, &cid, &meta_v1);
        let before = env.events().all().len();

        let meta_v2 = ContractMeta {
            version: 2,
            ..meta_v1
        };
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
}
