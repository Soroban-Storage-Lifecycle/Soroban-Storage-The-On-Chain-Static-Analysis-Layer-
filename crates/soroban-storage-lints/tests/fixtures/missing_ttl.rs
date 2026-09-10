//! Violations: write paths without TTL extension.
use soroban_sdk::Env;

pub fn store_without_bump(env: Env, key: soroban_sdk::Symbol, value: i128) {
    // Persistent write with no TTL management anywhere in this function.
    env.storage().persistent().set(&key, &value);
}

pub fn update_without_bump(env: Env, key: soroban_sdk::Symbol) {
    env.storage().persistent().update(&key, |v: i128| v + 1);
}

pub fn temporary_write_with_bump(env: Env, key: soroban_sdk::Symbol, value: u64) {
    // Temporary write that DOES extend TTL later in the same function: clean.
    env.storage().temporary().set(&key, &value);
    env.storage().temporary().extend_ttl(&key, 17_280, 518_400);
}