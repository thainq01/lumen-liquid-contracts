//! Typed accessors over Soroban storage. Keeps the contract entry points free
//! of `storage().instance().get(...)` boilerplate and consolidates the
//! "instance vs persistent" decision in one place.
//!
//! Storage layout:
//! * **Instance**: admin, position_manager, max_pos_usdc, pairs_count.
//!   These are touched by almost every entry point — keeping them in
//!   instance storage means one bumped read.
//! * **Persistent**: PairInfo, Group, RolloverState, FundingState, PairOi,
//!   per-pair depth. Sized by number of pairs/groups, lifetimes bumped by
//!   the contract's TTL extension policy (added in Phase 2).

use soroban_sdk::{Address, Env};

use crate::errors::PairRegistryError;
use crate::types::{DataKey, FundingState, Group, PairInfo, PairOi, RolloverState};

// ───────────────────────── TTL policy ─────────────────────────
// Extend persistent + instance entries by a configurable number of ledgers on
// every touch. Stored in instance storage so the admin can retune it (e.g. when
// the network's average ledger close time changes) without a contract upgrade.
// Default ≈ 7 days at ~5s/ledger.
const DEFAULT_TTL_EXTEND_LEDGERS: u32 = 120_960;

pub fn read_ttl_extend_ledgers(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get(&DataKey::TtlExtendLedgers)
        .unwrap_or(DEFAULT_TTL_EXTEND_LEDGERS)
}

pub fn write_ttl_extend_ledgers(env: &Env, ledgers: u32) {
    env.storage()
        .instance()
        .set(&DataKey::TtlExtendLedgers, &ledgers);
}

/// Bump the shared instance entry. Call once per mutating entry point.
pub fn bump_instance(env: &Env) {
    let n = read_ttl_extend_ledgers(env);
    env.storage().instance().extend_ttl(n, n);
}

fn bump_persistent(env: &Env, key: &DataKey) {
    let n = read_ttl_extend_ledgers(env);
    env.storage().persistent().extend_ttl(key, n, n);
}

// ───────────────────────── instance ─────────────────────────

pub fn read_admin(env: &Env) -> Result<Address, PairRegistryError> {
    env.storage()
        .instance()
        .get(&DataKey::Admin)
        .ok_or(PairRegistryError::NotInitialized)
}

pub fn write_admin(env: &Env, admin: &Address) {
    env.storage().instance().set(&DataKey::Admin, admin);
}

pub fn read_position_manager(env: &Env) -> Result<Address, PairRegistryError> {
    env.storage()
        .instance()
        .get(&DataKey::PositionManager)
        .ok_or(PairRegistryError::NotInitialized)
}

pub fn write_position_manager(env: &Env, pm: &Address) {
    env.storage().instance().set(&DataKey::PositionManager, pm);
}

pub fn read_max_pos_usdc(env: &Env) -> i128 {
    env.storage()
        .instance()
        .get(&DataKey::MaxPosUsdc)
        .unwrap_or(0)
}

pub fn write_max_pos_usdc(env: &Env, value: i128) {
    env.storage().instance().set(&DataKey::MaxPosUsdc, &value);
}

pub fn read_pairs_count(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get(&DataKey::PairsCount)
        .unwrap_or(0)
}

pub fn write_pairs_count(env: &Env, value: u32) {
    env.storage().instance().set(&DataKey::PairsCount, &value);
}

pub fn is_initialized(env: &Env) -> bool {
    env.storage().instance().has(&DataKey::Admin)
}

// ───────────────────────── persistent: pair / group ─────────────────────────

pub fn has_pair(env: &Env, pair_index: u32) -> bool {
    env.storage().persistent().has(&DataKey::Pair(pair_index))
}

pub fn read_pair(env: &Env, pair_index: u32) -> Result<PairInfo, PairRegistryError> {
    let key = DataKey::Pair(pair_index);
    let v: Option<PairInfo> = env.storage().persistent().get(&key);
    if v.is_some() {
        bump_persistent(env, &key);
    }
    v.ok_or(PairRegistryError::PairNotFound)
}

pub fn write_pair(env: &Env, pair_index: u32, pair: &PairInfo) {
    let key = DataKey::Pair(pair_index);
    env.storage().persistent().set(&key, pair);
    bump_persistent(env, &key);
}

pub fn has_group(env: &Env, group_index: u32) -> bool {
    env.storage().persistent().has(&DataKey::Group(group_index))
}

pub fn read_group(env: &Env, group_index: u32) -> Result<Group, PairRegistryError> {
    let key = DataKey::Group(group_index);
    let v: Option<Group> = env.storage().persistent().get(&key);
    if v.is_some() {
        bump_persistent(env, &key);
    }
    v.ok_or(PairRegistryError::GroupNotFound)
}

pub fn write_group(env: &Env, group_index: u32, group: &Group) {
    let key = DataKey::Group(group_index);
    env.storage().persistent().set(&key, group);
    bump_persistent(env, &key);
}

// ───────────────────────── persistent: accumulators ─────────────────────────

pub fn read_rollover(env: &Env, pair_index: u32) -> RolloverState {
    let key = DataKey::Rollover(pair_index);
    let v: Option<RolloverState> = env.storage().persistent().get(&key);
    if v.is_some() {
        bump_persistent(env, &key);
    }
    v.unwrap_or_default()
}

pub fn write_rollover(env: &Env, pair_index: u32, state: &RolloverState) {
    let key = DataKey::Rollover(pair_index);
    env.storage().persistent().set(&key, state);
    bump_persistent(env, &key);
}

pub fn read_funding(env: &Env, pair_index: u32) -> FundingState {
    let key = DataKey::Funding(pair_index);
    let v: Option<FundingState> = env.storage().persistent().get(&key);
    if v.is_some() {
        bump_persistent(env, &key);
    }
    v.unwrap_or_default()
}

pub fn write_funding(env: &Env, pair_index: u32, state: &FundingState) {
    let key = DataKey::Funding(pair_index);
    env.storage().persistent().set(&key, state);
    bump_persistent(env, &key);
}

pub fn read_oi(env: &Env, pair_index: u32) -> PairOi {
    let key = DataKey::OI(pair_index);
    let v: Option<PairOi> = env.storage().persistent().get(&key);
    if v.is_some() {
        bump_persistent(env, &key);
    }
    v.unwrap_or_default()
}

pub fn write_oi(env: &Env, pair_index: u32, oi: &PairOi) {
    let key = DataKey::OI(pair_index);
    env.storage().persistent().set(&key, oi);
    bump_persistent(env, &key);
}

pub fn read_depth(env: &Env, pair_index: u32) -> i128 {
    let key = DataKey::Depth(pair_index);
    let v: Option<i128> = env.storage().persistent().get(&key);
    if v.is_some() {
        bump_persistent(env, &key);
    }
    v.unwrap_or(0)
}

pub fn write_depth(env: &Env, pair_index: u32, value: i128) {
    let key = DataKey::Depth(pair_index);
    env.storage().persistent().set(&key, &value);
    bump_persistent(env, &key);
}
